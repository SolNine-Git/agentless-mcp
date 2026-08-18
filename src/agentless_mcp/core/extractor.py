"""tree-sitter symbol extraction across the supported language table.

Parses source files and extracts function, class, method, and constant
symbols.  Ported from the mcp-local extractor; the extraction logic, the
`LANGUAGE_CONFIGS` table and `SUPPORTED_EXTENSIONS` are unchanged, and the
ported tests are the tripwire that says so.

Language support is registry-driven: every language is wired in one place --
the registry built by `_build_registry` -- which pairs the grammar name with
the symbol- and import-extraction handlers for that language.  A single
dispatch consults it, so adding a language is a one-entry change instead of
edits kept in sync across separate ladders.  Languages with unusual ASTs
(Python, Rust, C/C++, Lua) name dedicated handlers; the rest name the generic
handlers driven by their `LANGUAGE_CONFIGS` entry.

The twelve per-grammar loader functions the original carried are gone: this
package gets every grammar from `core.grammars`, which owns fetching,
warmed-state and degradation.
"""

import logging
import re
from collections.abc import Callable
from dataclasses import dataclass
from functools import partial
from pathlib import Path
from typing import ClassVar

from tree_sitter import Language, Node, Parser

from agentless_mcp.core import grammars
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.symbols import ASTSymbol, SymbolKind

# Handlers normalised for the registry: both take the parsed root, the source
# bytes, the module path, and the accumulator list they append to.
SymbolHandler = Callable[[Node, bytes, str, list[ASTSymbol]], None]
ImportHandler = Callable[[Node, bytes, str, list[ImportStatement]], None]

logger = logging.getLogger(__name__)

_UPPER_CASE_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")

# Signatures are an index, not a rendering of the source: long initialisers and
# type aliases are truncated so one pathological literal cannot dominate a view.
_MAX_VALUE_CHARS = 80
_MAX_TYPE_ALIAS_CHARS = 120
# The shortest string literal that still has a quote at each end.
_MIN_QUOTED_LITERAL_CHARS = 2

# Every grammar in the table spells a plain name `identifier`; the languages
# with richer name node types override this in their LanguageConfig entry.
DEFAULT_IDENTIFIER_NODE_TYPES: tuple[str, ...] = ("identifier",)

# The ECMAScript family (js/ts/tsx) shares one set: property and shorthand
# property names are how a member reference is spelled, so leaving them out
# would make `this.total` invisible to the reference pass.
_ECMASCRIPT_IDENTIFIERS: tuple[str, ...] = (
    "identifier",
    "type_identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
)


def _truncate(text: str, limit: int) -> str:
    """Return ``text`` capped at ``limit`` characters, ellipsis included."""
    if len(text) > limit:
        return text[: limit - 3] + "..."
    return text


@dataclass(frozen=True)
class LanguageConfig:
    """
    Declares tree-sitter node types for generic symbol extraction.

    Used by _extract_generic_symbols() and _extract_generic_imports() to
    traverse AST nodes without language-specific code.  Languages that need
    richer extraction (Python, Rust) bypass this and use dedicated methods.
    """

    function_node_types: tuple[str, ...]
    class_node_types: tuple[str, ...]
    import_node_types: tuple[str, ...]
    # Field name on a function/class node that holds the identifier.
    # Set to None for languages where the name must be found by walking children.
    name_field: str | None = "name"
    # For imports: which field (or child type) contains the module path text.
    import_path_field: str | None = None
    # Node type of the identifier that holds the import path (e.g. "string").
    import_path_node_type: str | None = "string"
    # Leaf node types that name something: the raw material of the reference
    # pass in `core.refs`. The default covers the languages whose grammars
    # spell every name `identifier`; the entries below override it.
    identifier_node_types: tuple[str, ...] = DEFAULT_IDENTIFIER_NODE_TYPES


# ---------------------------------------------------------------------------
# Language configuration table
# ---------------------------------------------------------------------------
# Each entry drives the generic extractor.  Languages NOT in this table must
# have a dedicated _extract_<lang>_symbols() method.

LANGUAGE_CONFIGS: dict[str, LanguageConfig] = {
    "javascript": LanguageConfig(
        function_node_types=(
            "function_declaration",
            "generator_function_declaration",
            "method_definition",
        ),
        class_node_types=("class_declaration",),
        import_node_types=("import_statement",),
        name_field="name",
        import_path_field="source",
        import_path_node_type="string",
        identifier_node_types=_ECMASCRIPT_IDENTIFIERS,
    ),
    "typescript": LanguageConfig(
        function_node_types=(
            "function_declaration",
            "generator_function_declaration",
            "method_definition",
        ),
        class_node_types=("class_declaration", "interface_declaration"),
        import_node_types=("import_statement",),
        name_field="name",
        import_path_field="source",
        import_path_node_type="string",
        identifier_node_types=_ECMASCRIPT_IDENTIFIERS,
    ),
    "tsx": LanguageConfig(
        function_node_types=(
            "function_declaration",
            "generator_function_declaration",
            "method_definition",
        ),
        class_node_types=("class_declaration",),
        import_node_types=("import_statement",),
        name_field="name",
        import_path_field="source",
        import_path_node_type="string",
        identifier_node_types=_ECMASCRIPT_IDENTIFIERS,
    ),
    "go": LanguageConfig(
        function_node_types=("function_declaration", "method_declaration"),
        class_node_types=("type_declaration",),
        import_node_types=("import_spec",),
        name_field="name",
        import_path_node_type="interpreted_string_literal",
        identifier_node_types=(
            "identifier",
            "type_identifier",
            "field_identifier",
            "package_identifier",
        ),
    ),
    "lua": LanguageConfig(
        # tree-sitter-lua emits `function_declaration` for `local function foo()`
        # too, so a separate `local_function` node type never matches.
        function_node_types=("function_declaration",),
        class_node_types=(),
        import_node_types=(),  # require() calls detected separately
        name_field="name",
    ),
    "bash": LanguageConfig(
        function_node_types=("function_definition",),
        class_node_types=(),
        import_node_types=(),  # source/. builtins detected separately
        name_field="name",
        # A shell has no `identifier` node: a name is either a variable or the
        # bare word that names the command being run.
        identifier_node_types=("variable_name", "word"),
    ),
    "java": LanguageConfig(
        function_node_types=("method_declaration", "constructor_declaration"),
        class_node_types=("class_declaration", "interface_declaration", "enum_declaration"),
        import_node_types=("import_declaration",),
        name_field="name",
        import_path_node_type="scoped_identifier",
        identifier_node_types=("identifier", "type_identifier"),
    ),
    "ruby": LanguageConfig(
        function_node_types=("method", "singleton_method"),
        class_node_types=("class", "module"),
        import_node_types=(),  # require calls detected separately
        name_field="name",
        identifier_node_types=("identifier", "constant"),
    ),
}

# C and C++ are handled by dedicated methods due to their nested declarator
# structure (function name lives in declarator.declarator, not a direct field).


@dataclass(frozen=True)
class _GenericBinding:
    """The per-language half of the generic traversal, bound at registry time."""

    cfg: LanguageConfig
    language: str


@dataclass(frozen=True)
class _GenericWalk:
    """The per-call half: where symbols come from and where they accumulate."""

    cfg: LanguageConfig
    language: str
    module_path: str
    symbols: list[ASTSymbol]


@dataclass(frozen=True)
class _LanguageSpec:
    """One language's wiring: grammar name plus both extraction handlers.

    The single home a language is declared in.  `grammar` is the name
    `core.grammars` resolves; `extract_symbols` and `extract_imports` are
    normalised handlers (see SymbolHandler / ImportHandler).  The registry is
    what a single dispatch consults instead of two hand-synced if/elif
    ladders.
    """

    grammar: str
    extract_symbols: SymbolHandler
    extract_imports: ImportHandler


class TreeSitterExtractor:
    """Extract AST symbols from source files using tree-sitter."""

    SUPPORTED_EXTENSIONS: ClassVar[dict[str, str]] = {
        # Python (dedicated extractor)
        ".py": "python",
        # Rust (dedicated extractor)
        ".rs": "rust",
        # JavaScript / TypeScript (generic extractor)
        ".js": "javascript",
        ".mjs": "javascript",
        ".cjs": "javascript",
        ".ts": "typescript",
        ".tsx": "tsx",
        # Go (generic extractor)
        ".go": "go",
        # Lua (generic extractor — Neovim plugins)
        ".lua": "lua",
        # C / C++ (dedicated extractor)
        ".c": "c",
        ".h": "c",
        ".cc": "cpp",
        ".cpp": "cpp",
        ".cxx": "cpp",
        ".hpp": "cpp",
        ".hxx": "cpp",
        # Shell / Bash (generic extractor)
        ".sh": "bash",
        ".bash": "bash",
        # JVM / Ruby (generic extractor)
        ".java": "java",
        ".rb": "ruby",
    }

    def __init__(self) -> None:
        self._registry: dict[str, _LanguageSpec] = self._build_registry()

    # ------------------------------------------------------------------
    # Language registry (single dispatch table)
    # ------------------------------------------------------------------

    def _build_registry(self) -> dict[str, _LanguageSpec]:
        """Wire every language to its grammar and both extraction handlers.

        Built once per instance; cheap (partials and bound methods, no grammar
        load).  This is the single place a language is dispatched -- the two
        public extraction methods and the parser lookup all read from it, so a
        language cannot be handled by one chain and missed by another.
        """

        def generic_symbols(language: str) -> SymbolHandler:
            return partial(
                self._extract_generic_symbols,
                binding=_GenericBinding(cfg=LANGUAGE_CONFIGS[language], language=language),
            )

        def generic_imports(language: str) -> ImportHandler:
            return partial(self._extract_generic_imports, cfg=LANGUAGE_CONFIGS[language])

        return {
            "python": _LanguageSpec(
                "python", self._extract_python_symbols, self._extract_python_imports
            ),
            "rust": _LanguageSpec("rust", self._extract_rust_symbols, self._extract_rust_imports),
            "c": _LanguageSpec(
                "c", partial(self._extract_c_symbols, language="c"), self._extract_c_imports
            ),
            "cpp": _LanguageSpec(
                "cpp",
                partial(self._extract_c_symbols, language="cpp"),
                self._extract_c_imports,
            ),
            "lua": _LanguageSpec("lua", self._extract_lua_symbols, self._extract_require_imports),
            "ruby": _LanguageSpec("ruby", generic_symbols("ruby"), self._extract_require_imports),
            "bash": _LanguageSpec("bash", generic_symbols("bash"), self._extract_bash_imports),
            "javascript": _LanguageSpec(
                "javascript", generic_symbols("javascript"), generic_imports("javascript")
            ),
            "typescript": _LanguageSpec(
                "typescript", generic_symbols("typescript"), generic_imports("typescript")
            ),
            # The pack ships a real tsx grammar, so tsx no longer borrows the
            # javascript one as it did in mcp-local.
            "tsx": _LanguageSpec("tsx", generic_symbols("tsx"), generic_imports("tsx")),
            "go": _LanguageSpec("go", generic_symbols("go"), generic_imports("go")),
            "java": _LanguageSpec("java", generic_symbols("java"), generic_imports("java")),
        }

    # ------------------------------------------------------------------
    # Parser caching
    # ------------------------------------------------------------------

    def get_parser(self, language: str) -> Parser:
        """Return the memoized parser for a supported language."""
        return grammars.get_parser(self._grammar_of(language))

    def load_language(self, language: str) -> Language:
        """Load the tree-sitter Language object for the given language name."""
        return grammars.get_language(self._grammar_of(language))

    def _grammar_of(self, language: str) -> str:
        """Map a supported language name onto its grammar name."""
        spec = self._registry.get(language)
        if spec is None:
            msg = f"Unsupported language: {language}"
            raise ValueError(msg)
        return spec.grammar

    # ------------------------------------------------------------------
    # Public symbol extraction API
    # ------------------------------------------------------------------

    def extract_symbols(self, file_path: Path) -> list[ASTSymbol]:
        """Extract all symbols from a source file."""
        suffix = file_path.suffix
        language = self.SUPPORTED_EXTENSIONS.get(suffix)
        if language is None:
            return []

        try:
            source = file_path.read_bytes()
        except (OSError, PermissionError) as e:
            logger.warning("Cannot read %s: %s", file_path, e)
            return []

        module_path = str(file_path)
        return self.extract_from_source(
            source.decode("utf-8", errors="replace"), language, module_path
        )

    def extract_from_source(self, source: str, language: str, module_path: str) -> list[ASTSymbol]:
        """Extract symbols from source string.

        An unsupported language yields no symbols.  A supported language whose
        grammar is unavailable raises `LanguageUnavailable` from
        `core.grammars`: that is a degradation the caller has to see, not an
        empty file.
        """
        try:
            parser = self.get_parser(language)
        except ValueError as e:
            logger.warning("Unsupported language %s (%s): %s", language, module_path, e)
            return []

        tree = parser.parse(bytes(source, "utf-8"))
        source_bytes = bytes(source, "utf-8")

        # get_parser succeeded, so the language is registered.
        symbols: list[ASTSymbol] = []
        self._registry[language].extract_symbols(tree.root_node, source_bytes, module_path, symbols)
        return symbols

    # ------------------------------------------------------------------
    # Public import extraction API
    # ------------------------------------------------------------------

    def extract_imports(self, file_path: Path) -> list[ImportStatement]:
        """Extract all import statements from a source file."""
        suffix = file_path.suffix
        language = self.SUPPORTED_EXTENSIONS.get(suffix)
        if language is None:
            return []

        try:
            source = file_path.read_bytes()
        except (OSError, PermissionError) as e:
            logger.warning("Cannot read %s: %s", file_path, e)
            return []

        module_path = str(file_path)
        return self.extract_imports_from_source(
            source.decode("utf-8", errors="replace"), language, module_path
        )

    def extract_imports_from_source(
        self, source: str, language: str, module_path: str
    ) -> list[ImportStatement]:
        """Extract import statements from source string.

        Same contract as `extract_from_source`: unsupported means empty,
        unavailable grammar means `LanguageUnavailable`.
        """
        try:
            parser = self.get_parser(language)
        except ValueError as e:
            logger.warning("Unsupported language %s (%s): %s", language, module_path, e)
            return []

        tree = parser.parse(bytes(source, "utf-8"))
        source_bytes = bytes(source, "utf-8")

        # get_parser succeeded, so the language is registered.
        imports: list[ImportStatement] = []
        self._registry[language].extract_imports(tree.root_node, source_bytes, module_path, imports)
        return imports

    # ------------------------------------------------------------------
    # Generic symbol / import extraction (table-driven)
    # ------------------------------------------------------------------

    def _extract_generic_symbols(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
        binding: _GenericBinding,
    ) -> None:
        """Generic symbol traversal driven by LanguageConfig.

        The walk descends through every node that is not itself a declaration,
        which is what makes `export class X` and `export function f` visible:
        those are wrapped in an ``export_statement``, and a loop over the
        root's direct children never reaches inside one. It stops descending
        at a matched function, because a closure defined inside a body is part
        of that body, not a second top-level symbol; a matched class keeps its
        existing body recursion, so its methods carry the class as parent.
        """
        walk = _GenericWalk(
            cfg=binding.cfg,
            language=binding.language,
            module_path=module_path,
            symbols=symbols,
        )
        self._visit_generic_children(root, source, walk, parent="")

    def _visit_generic_children(
        self,
        node: Node,
        source: bytes,
        walk: _GenericWalk,
        parent: str,
    ) -> None:
        """Visit every child of ``node`` at the same nesting level."""
        for child in node.children:
            self._visit_generic_node(child, source, walk, parent)

    def _visit_generic_node(
        self,
        node: Node,
        source: bytes,
        walk: _GenericWalk,
        parent: str,
    ) -> None:
        """Visit a single node and emit a symbol if it matches the config."""
        cfg = walk.cfg
        language = walk.language
        module_path = walk.module_path
        symbols = walk.symbols
        if node.type in cfg.function_node_types:
            name = self._generic_name(node, source, cfg.name_field)
            if name:
                kind = SymbolKind.METHOD if parent else SymbolKind.FUNCTION
                symbols.append(
                    ASTSymbol(
                        name=name,
                        kind=kind,
                        module_path=module_path,
                        line_number=node.start_point[0] + 1,
                        end_line_number=node.end_point[0] + 1,
                        signature=self._generic_signature(node, source, name),
                        docstring="",
                        parent_class=parent,
                        decorators=(),
                        bases=(),
                        language=language,
                        is_public=not name.startswith("_"),
                        is_async=False,
                    )
                )
        elif node.type in cfg.class_node_types:
            name = self._generic_name(node, source, cfg.name_field)
            if name:
                symbols.append(
                    ASTSymbol(
                        name=name,
                        kind=SymbolKind.CLASS,
                        module_path=module_path,
                        line_number=node.start_point[0] + 1,
                        end_line_number=node.end_point[0] + 1,
                        signature=f"class {name}",
                        docstring="",
                        parent_class="",
                        decorators=(),
                        bases=(),
                        language=language,
                        is_public=not name.startswith("_"),
                        is_async=False,
                    )
                )
                # Recurse into class body for methods
                body = node.child_by_field_name("body")
                if body:
                    self._visit_generic_children(body, source, walk, parent=name)
        else:
            # Not a declaration: a wrapper (export_statement, a block, an
            # expression) that may still hold one.
            self._visit_generic_children(node, source, walk, parent)

    def _generic_signature(self, node: Node, source: bytes, name: str) -> str:
        """Render `fn name(params) -> result` from whatever fields exist.

        A bare `fn name` tells a reader nothing about arity or types, and the
        map is read as code. Parameter and result text comes straight from the
        source; collapsing it onto one line and capping its length is
        `ASTSymbol`'s job, so every handler gets the same treatment.
        """
        params_node = node.child_by_field_name("parameters")
        params = self._node_text(params_node, source) if params_node else "()"

        result_node = node.child_by_field_name("return_type") or node.child_by_field_name("result")
        # TypeScript's return_type is a `type_annotation`, so its text arrives
        # as ": string"; the arrow already says what the colon would.
        result_text = self._node_text(result_node, source).lstrip(": ") if result_node else ""
        result = f" -> {result_text}" if result_text else ""

        return f"fn {name}{params}{result}"

    def _generic_name(self, node: Node, source: bytes, name_field: str | None) -> str:
        """Extract the identifier name from a node using the configured field."""
        if name_field:
            name_node = node.child_by_field_name(name_field)
            if name_node:
                return self._node_text(name_node, source)
        # Fallback: first named identifier child
        for child in node.children:
            if child.type in ("identifier", "type_identifier", "property_identifier", "word"):
                return self._node_text(child, source)
        return ""

    def _extract_generic_imports(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        imports: list[ImportStatement],
        cfg: LanguageConfig,
    ) -> None:
        """Generic import extraction driven by LanguageConfig."""
        _ = module_path
        for node in root.children:
            self._collect_import_nodes(node, source, imports, cfg)

    def _collect_import_nodes(
        self,
        node: Node,
        source: bytes,
        imports: list[ImportStatement],
        cfg: LanguageConfig,
    ) -> None:
        """Recursively collect import nodes (some languages nest them in blocks)."""
        if node.type in cfg.import_node_types:
            path = self._extract_import_path(node, source, cfg)
            if path:
                imports.append(
                    ImportStatement(
                        module=path,
                        names=(),
                        is_relative=path.startswith("."),
                        relative_level=0,
                        line_number=node.start_point[0] + 1,
                        resolved_path="",
                    )
                )
        # Recurse for block imports (e.g. Go import blocks)
        for child in node.children:
            self._collect_import_nodes(child, source, imports, cfg)

    def _extract_import_path(
        self,
        node: Node,
        source: bytes,
        cfg: LanguageConfig,
    ) -> str:
        """Extract the module path string from an import node."""
        # Try named field first
        if cfg.import_path_field:
            path_node = node.child_by_field_name(cfg.import_path_field)
            if path_node:
                return self._strip_quotes(self._node_text(path_node, source))

        # Walk children looking for the expected string node type
        target_type = cfg.import_path_node_type or "string"
        for child in node.children:
            if child.type == target_type:
                return self._strip_quotes(self._node_text(child, source))
            # One level of nesting (e.g. import_statement > string_fragment)
            for grandchild in child.children:
                if grandchild.type == target_type:
                    return self._strip_quotes(self._node_text(grandchild, source))
        return ""

    # ------------------------------------------------------------------
    # Lua (dedicated — assigned functions and module tables)
    # ------------------------------------------------------------------

    def _extract_lua_symbols(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
    ) -> None:
        """Extract Lua symbols including assigned functions and module tables.

        Handles patterns the generic extractor cannot:
        - ``M.greet = function(name) ... end``  (assignment with function_definition value)
        - ``local on_attach = function(...) ... end``  (variable_declaration with function value)
        - ``local M = {}``  (module table declarations)
        - ``local CONSTANT = 42``  (uppercase constants)
        """
        for node in root.children:
            if node.type == "function_declaration":
                name = self._lua_function_name(node, source)
                if name:
                    symbols.append(
                        self._lua_symbol(
                            name,
                            SymbolKind.FUNCTION,
                            module_path,
                            node,
                        )
                    )

            elif node.type == "assignment_statement":
                self._extract_lua_assignment(node, source, module_path, symbols)

            elif node.type == "variable_declaration":
                self._extract_lua_variable_decl(node, source, module_path, symbols)

    def _extract_lua_assignment(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
    ) -> None:
        """Handle top-level ``M.foo = function() end`` or ``M.foo = value``."""
        var_list = node.child_by_field_name("values")
        name_list = node.child_by_field_name("variables")
        if name_list is None:
            # Fallback: walk children
            for child in node.children:
                if child.type == "variable_list":
                    name_list = child
                elif child.type == "expression_list":
                    var_list = child

        if name_list is None:
            return

        name = self._lua_lhs_name(name_list, source)
        if not name:
            return

        has_func_value = var_list is not None and any(
            c.type == "function_definition" for c in var_list.children
        )

        if has_func_value:
            assert var_list is not None  # implied by has_func_value
            params = self._lua_assigned_func_params(var_list, source)
            symbols.append(
                self._lua_symbol(
                    name,
                    SymbolKind.FUNCTION,
                    module_path,
                    node,
                    signature=f"fn {name}{params}",
                )
            )

    def _extract_lua_variable_decl(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
    ) -> None:
        """Handle ``local M = {}`` and ``local on_attach = function() end``."""
        assign = None
        for child in node.children:
            if child.type == "assignment_statement":
                assign = child
                break
        if assign is None:
            return

        name_list = None
        val_list = None
        for child in assign.children:
            if child.type == "variable_list":
                name_list = child
            elif child.type == "expression_list":
                val_list = child

        if name_list is None:
            return

        name = self._lua_lhs_name(name_list, source)
        if not name:
            return

        has_func_value = val_list is not None and any(
            c.type == "function_definition" for c in val_list.children
        )
        has_table_value = val_list is not None and any(
            c.type == "table_constructor" for c in val_list.children
        )

        if has_func_value:
            assert val_list is not None  # implied by has_func_value
            params = self._lua_assigned_func_params(val_list, source)
            symbols.append(
                self._lua_symbol(
                    name,
                    SymbolKind.FUNCTION,
                    module_path,
                    node,
                    signature=f"local fn {name}{params}",
                )
            )
        elif has_table_value:
            symbols.append(
                self._lua_symbol(
                    name,
                    SymbolKind.CLASS,
                    module_path,
                    node,
                    signature=f"local {name} = {{}}",
                )
            )
        elif _UPPER_CASE_RE.match(name):
            symbols.append(
                self._lua_symbol(
                    name,
                    SymbolKind.CONSTANT,
                    module_path,
                    node,
                    signature=f"local {name}",
                )
            )

    def _lua_function_name(self, node: Node, source: bytes) -> str:
        """Get name from a function_declaration (handles dot_index_expression)."""
        name_node = node.child_by_field_name("name")
        if name_node:
            return self._node_text(name_node, source)
        return ""

    def _lua_lhs_name(self, name_list: Node, source: bytes) -> str:
        """Extract the name from a variable_list node."""
        for child in name_list.children:
            if child.type in ("identifier", "dot_index_expression"):
                return self._node_text(child, source)
        return ""

    def _lua_assigned_func_params(self, val_list: Node, source: bytes) -> str:
        """Extract parameter list from an assigned function_definition."""
        for child in val_list.children:
            if child.type == "function_definition":
                params = child.child_by_field_name("parameters")
                if params:
                    return self._node_text(params, source)
        return "()"

    @staticmethod
    def _lua_symbol(
        name: str,
        kind: SymbolKind,
        module_path: str,
        node: Node,
        signature: str = "",
    ) -> ASTSymbol:
        if not signature:
            signature = f"fn {name}"
        return ASTSymbol(
            name=name,
            kind=kind,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=signature,
            docstring="",
            parent_class="",
            decorators=(),
            bases=(),
            language="lua",
            is_public=not name.startswith("_"),
            is_async=False,
        )

    # ------------------------------------------------------------------
    # C / C++ (dedicated — complex declarator structure)
    # ------------------------------------------------------------------

    def _extract_c_symbols(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
        language: str,
    ) -> None:
        """Extract function and struct symbols from C/C++ AST."""
        for node in root.children:
            if node.type == "function_definition":
                name = self._c_function_name(node, source)
                if name:
                    symbols.append(
                        ASTSymbol(
                            name=name,
                            kind=SymbolKind.FUNCTION,
                            module_path=module_path,
                            line_number=node.start_point[0] + 1,
                            end_line_number=node.end_point[0] + 1,
                            signature=f"fn {name}",
                            docstring="",
                            parent_class="",
                            decorators=(),
                            bases=(),
                            language=language,
                            is_public=True,
                            is_async=False,
                        )
                    )
            elif node.type in ("struct_specifier", "class_specifier", "enum_specifier"):
                name_node = node.child_by_field_name("name")
                if name_node:
                    name = self._node_text(name_node, source)
                    symbols.append(
                        ASTSymbol(
                            name=name,
                            kind=SymbolKind.CLASS,
                            module_path=module_path,
                            line_number=node.start_point[0] + 1,
                            end_line_number=node.end_point[0] + 1,
                            signature=f"{node.type.split('_')[0]} {name}",
                            docstring="",
                            parent_class="",
                            decorators=(),
                            bases=(),
                            language=language,
                            is_public=True,
                            is_async=False,
                        )
                    )

    def _c_function_name(self, node: Node, source: bytes) -> str:
        """
        Extract function name from a C function_definition.

        C AST structure:
          function_definition
            declarator: function_declarator
              declarator: identifier  ← target
              parameters: ...
        """
        declarator = node.child_by_field_name("declarator")
        if declarator is None:
            return ""
        # Unwrap pointer_declarator / reference_declarator layers
        while declarator and declarator.type in (
            "pointer_declarator",
            "reference_declarator",
            "abstract_declarator",
        ):
            inner = declarator.child_by_field_name("declarator")
            if inner is None:
                break
            declarator = inner
        if declarator.type == "function_declarator":
            inner = declarator.child_by_field_name("declarator")
            if inner:
                return self._node_text(inner, source)
        return self._node_text(declarator, source)

    def _extract_c_imports(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        imports: list[ImportStatement],
    ) -> None:
        """Extract #include directives as import statements."""
        _ = module_path
        for node in root.children:
            if node.type == "preproc_include":
                for child in node.children:
                    if child.type in ("string_literal", "system_lib_string"):
                        path = self._strip_quotes(self._node_text(child, source).strip("<>"))
                        imports.append(
                            ImportStatement(
                                module=path,
                                names=(),
                                is_relative=not self._node_text(child, source).startswith("<"),
                                relative_level=0,
                                line_number=node.start_point[0] + 1,
                                resolved_path="",
                            )
                        )
                        break

    # ------------------------------------------------------------------
    # Require-style imports (Lua / Ruby)
    # ------------------------------------------------------------------

    def _extract_require_imports(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        imports: list[ImportStatement],
    ) -> None:
        """
        Extract require("module") / require 'module' calls.

        Walks the entire tree looking for call nodes where the function
        is named 'require'.
        """
        _ = module_path
        self._walk_require(root, source, imports)

    def _walk_require(self, node: Node, source: bytes, imports: list[ImportStatement]) -> None:
        """Depth-first walk collecting require() calls."""
        if node.type in ("call", "call_expression", "function_call"):
            fn_node = node.child_by_field_name("function") or (
                node.children[0] if node.children else None
            )
            if fn_node and self._node_text(fn_node, source) == "require":
                args = node.child_by_field_name("arguments")
                if args:
                    for child in args.children:
                        if child.type in ("string", "string_literal"):
                            path = self._strip_quotes(self._node_text(child, source))
                            if path:
                                imports.append(
                                    ImportStatement(
                                        module=path,
                                        names=(),
                                        is_relative=path.startswith("."),
                                        relative_level=0,
                                        line_number=node.start_point[0] + 1,
                                        resolved_path="",
                                    )
                                )
                            break
        for child in node.children:
            self._walk_require(child, source, imports)

    # ------------------------------------------------------------------
    # Bash source imports
    # ------------------------------------------------------------------

    def _extract_bash_imports(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        imports: list[ImportStatement],
    ) -> None:
        """Extract 'source' and '.' (dot) commands as import statements."""
        _ = module_path
        self._walk_bash_source(root, source, imports)

    def _walk_bash_source(self, node: Node, source: bytes, imports: list[ImportStatement]) -> None:
        """Depth-first walk collecting source/dot commands."""
        if node.type == "command":
            children = list(node.children)
            if children:
                cmd_name = self._node_text(children[0], source)
                if cmd_name in ("source", ".") and len(children) > 1:
                    path = self._node_text(children[1], source).strip("'\"")
                    imports.append(
                        ImportStatement(
                            module=path,
                            names=(),
                            is_relative=not path.startswith("/"),
                            relative_level=0,
                            line_number=node.start_point[0] + 1,
                            resolved_path="",
                        )
                    )
        for child in node.children:
            self._walk_bash_source(child, source, imports)

    # ------------------------------------------------------------------
    # Python extraction (unchanged)
    # ------------------------------------------------------------------

    def _extract_python_symbols(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
    ) -> None:
        """Extract symbols from a Python AST."""
        for child in root.children:
            if child.type == "function_definition":
                symbols.append(self._extract_function(child, source, module_path, parent_class=""))
            elif child.type == "class_definition":
                self._extract_class(child, source, module_path, symbols)
            elif child.type in ("expression_statement", "assignment"):
                sym = self._try_extract_constant(child, source, module_path)
                if sym is not None:
                    symbols.append(sym)
            elif child.type == "type_alias_statement":
                sym = self._extract_type_alias(child, source, module_path)
                if sym is not None:
                    symbols.append(sym)
            elif child.type == "decorated_definition":
                self._extract_decorated(child, source, module_path, symbols, parent_class="")

    def _extract_decorated(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
        parent_class: str,
    ) -> None:
        """Extract a decorated function or class definition."""
        decorators = self._get_decorators(node, source)
        definition = None
        for child in node.children:
            if child.type in ("function_definition", "class_definition"):
                definition = child
                break

        if definition is None:
            return

        if definition.type == "function_definition":
            sym = self._extract_function(
                definition, source, module_path, parent_class, extra_decorators=decorators
            )
            symbols.append(sym)
        elif definition.type == "class_definition":
            self._extract_class(
                definition, source, module_path, symbols, extra_decorators=decorators
            )

    def _extract_function(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        parent_class: str,
        extra_decorators: tuple[str, ...] = (),
    ) -> ASTSymbol:
        """Extract a function or method symbol."""
        name_node = node.child_by_field_name("name")
        name = self._node_text(name_node, source) if name_node else "<unknown>"

        params_node = node.child_by_field_name("parameters")
        params_text = self._node_text(params_node, source) if params_node else "()"

        return_node = node.child_by_field_name("return_type")
        return_text = f" -> {self._node_text(return_node, source)}" if return_node else ""

        is_async = False
        if node.parent and node.parent.type == "decorated_definition":
            is_async = any(c.type == "async" for c in node.parent.children)
        if not is_async:
            end = min(node.start_byte + 10, node.end_byte)
            prefix_text = source[node.start_byte : end].decode("utf-8", errors="replace")
            if "async" in prefix_text:
                is_async = True

        prefix = "async def" if is_async else "def"
        signature = f"{prefix} {name}{params_text}{return_text}"

        decorators = extra_decorators or self._get_own_decorators(node, source)
        docstring = self._get_docstring(node, source)

        kind = SymbolKind.METHOD if parent_class else SymbolKind.FUNCTION

        return ASTSymbol(
            name=name,
            kind=kind,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=signature,
            docstring=docstring,
            parent_class=parent_class,
            decorators=decorators,
            bases=(),
            language="python",
            is_public=not name.startswith("_"),
            is_async=is_async,
        )

    def _extract_class(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
        extra_decorators: tuple[str, ...] = (),
    ) -> None:
        """Extract class and its methods."""
        name_node = node.child_by_field_name("name")
        class_name = self._node_text(name_node, source) if name_node else "<unknown>"

        bases = self._get_bases(node, source)
        decorators = extra_decorators or self._get_own_decorators(node, source)

        kind = self._classify_class(class_name, bases, decorators)

        superclasses_node = node.child_by_field_name("superclasses")
        bases_text = self._node_text(superclasses_node, source) if superclasses_node else ""
        signature = f"class {class_name}{bases_text}" if bases_text else f"class {class_name}"

        docstring = self._get_docstring(node, source)

        symbols.append(
            ASTSymbol(
                name=class_name,
                kind=kind,
                module_path=module_path,
                line_number=node.start_point[0] + 1,
                end_line_number=node.end_point[0] + 1,
                signature=signature,
                docstring=docstring,
                parent_class="",
                decorators=decorators,
                bases=bases,
                language="python",
                is_public=not class_name.startswith("_"),
                is_async=False,
            )
        )

        body = node.child_by_field_name("body")
        if body:
            for child in body.children:
                if child.type == "function_definition":
                    symbols.append(self._extract_function(child, source, module_path, class_name))
                elif child.type == "decorated_definition":
                    self._extract_decorated(
                        child, source, module_path, symbols, parent_class=class_name
                    )

    def _classify_class(
        self, name: str, bases: tuple[str, ...], decorators: tuple[str, ...]
    ) -> SymbolKind:
        """Determine specific SymbolKind for a class."""
        _ = name
        dec_names = {d.split("(")[0] for d in decorators}
        if "dataclass" in dec_names or "dataclasses.dataclass" in dec_names:
            return SymbolKind.DATACLASS
        if any("Protocol" in b for b in bases):
            return SymbolKind.PROTOCOL
        if any("Enum" in b or "StrEnum" in b or "IntEnum" in b for b in bases):
            return SymbolKind.ENUM
        return SymbolKind.CLASS

    def _try_extract_constant(
        self, node: Node, source: bytes, module_path: str
    ) -> ASTSymbol | None:
        """Try to extract a module-level constant (UPPER_CASE assignment).

        Grammar drift, 2026-08-18: the python grammar shipped by
        tree-sitter-language-pack 1.14.3 (ABI 14) no longer wraps module-level
        statements in an `expression_statement`, so the assignment arrives
        either as this node or as its first child.  Both shapes are accepted
        so the extractor keeps working against either revision.
        """
        child = node if node.type == "assignment" else None
        if child is None:
            if not node.children:
                return None
            child = node.children[0]
        if child.type != "assignment":
            return None

        left = child.child_by_field_name("left")
        if left is None or left.type != "identifier":
            return None

        name = self._node_text(left, source)
        if not _UPPER_CASE_RE.match(name):
            return None

        right = child.child_by_field_name("right")
        # A multi-line dict literal arrives verbatim here; `ASTSymbol` collapses
        # and caps the composed signature, so this only bounds the value.
        right_text = _truncate(self._node_text(right, source) if right else "", _MAX_VALUE_CHARS)

        return ASTSymbol(
            name=name,
            kind=SymbolKind.CONSTANT,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=f"{name} = {right_text}",
            docstring="",
            parent_class="",
            decorators=(),
            bases=(),
            language="python",
            is_public=not name.startswith("_"),
            is_async=False,
        )

    def _extract_type_alias(self, node: Node, source: bytes, module_path: str) -> ASTSymbol | None:
        """Extract a type alias (Python 3.12+ 'type X = ...')."""
        name_node = node.child_by_field_name("name")
        if name_node is None:
            return None

        name = self._node_text(name_node, source)
        full_text = _truncate(self._node_text(node, source), _MAX_TYPE_ALIAS_CHARS)

        return ASTSymbol(
            name=name,
            kind=SymbolKind.TYPE_ALIAS,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=full_text,
            docstring="",
            parent_class="",
            decorators=(),
            bases=(),
            language="python",
            is_public=not name.startswith("_"),
            is_async=False,
        )

    # ------------------------------------------------------------------
    # Rust extraction (unchanged)
    # ------------------------------------------------------------------

    def _extract_rust_symbols(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
    ) -> None:
        """Extract symbols from a Rust AST (top-level items only)."""
        for child in root.children:
            self._visit_rust_item(child, source, module_path, symbols, parent_class="")

    def _visit_rust_item(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
        parent_class: str,
    ) -> None:
        """Dispatch a single Rust AST item to the appropriate extractor."""
        if node.type == "function_item":
            symbols.append(self._extract_rust_function(node, source, module_path, parent_class))
        elif node.type in ("struct_item", "enum_item", "trait_item"):
            sym = self._extract_rust_adt(node, source, module_path)
            if sym is not None:
                symbols.append(sym)
        elif node.type == "impl_item":
            self._extract_rust_impl(node, source, module_path, symbols)
        elif node.type == "const_item":
            sym = self._extract_rust_const(node, source, module_path)
            if sym is not None:
                symbols.append(sym)
        elif node.type == "type_item":
            sym = self._extract_rust_type_alias(node, source, module_path)
            if sym is not None:
                symbols.append(sym)

    def _rust_is_public(self, node: Node) -> bool:
        """Return True if the node has a visibility_modifier child (pub/pub(crate)/etc.)."""
        return any(c.type == "visibility_modifier" for c in node.children)

    def _extract_rust_function(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        parent_class: str,
    ) -> ASTSymbol:
        """Extract a Rust function_item or method."""
        name_node = node.child_by_field_name("name")
        name = self._node_text(name_node, source) if name_node else "<unknown>"

        params_node = node.child_by_field_name("parameters")
        params_text = self._node_text(params_node, source) if params_node else "()"

        return_node = node.child_by_field_name("return_type")
        return_text = f" -> {self._node_text(return_node, source)}" if return_node else ""

        is_async = any(c.type == "async" for c in node.children)
        prefix = "async fn" if is_async else "fn"
        signature = f"{prefix} {name}{params_text}{return_text}"

        kind = SymbolKind.METHOD if parent_class else SymbolKind.FUNCTION

        return ASTSymbol(
            name=name,
            kind=kind,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=signature,
            docstring="",
            parent_class=parent_class,
            decorators=(),
            bases=(),
            language="rust",
            is_public=self._rust_is_public(node),
            is_async=is_async,
        )

    def _extract_rust_adt(
        self,
        node: Node,
        source: bytes,
        module_path: str,
    ) -> ASTSymbol | None:
        """Extract a struct, enum, or trait item."""
        name_node = node.child_by_field_name("name")
        if name_node is None:
            return None
        name = self._node_text(name_node, source)

        kind_map = {
            "struct_item": (SymbolKind.CLASS, "struct"),
            "enum_item": (SymbolKind.ENUM, "enum"),
            "trait_item": (SymbolKind.PROTOCOL, "trait"),
        }
        kind, keyword = kind_map[node.type]

        return ASTSymbol(
            name=name,
            kind=kind,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=f"{keyword} {name}",
            docstring="",
            parent_class="",
            decorators=(),
            bases=(),
            language="rust",
            is_public=self._rust_is_public(node),
            is_async=False,
        )

    def _extract_rust_impl(
        self,
        node: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
    ) -> None:
        """Extract methods from an impl block."""
        type_node = node.child_by_field_name("type")
        parent_class = self._node_text(type_node, source) if type_node else ""

        body = node.child_by_field_name("body")
        if body is None:
            return
        for child in body.children:
            if child.type == "function_item":
                symbols.append(
                    self._extract_rust_function(child, source, module_path, parent_class)
                )

    def _extract_rust_const(
        self,
        node: Node,
        source: bytes,
        module_path: str,
    ) -> ASTSymbol | None:
        """Extract a const_item."""
        name_node = node.child_by_field_name("name")
        if name_node is None:
            return None
        name = self._node_text(name_node, source)

        type_node = node.child_by_field_name("type")
        value_node = node.child_by_field_name("value")
        type_text = f": {self._node_text(type_node, source)}" if type_node else ""
        raw_value = self._node_text(value_node, source) if value_node else ""
        value_text = _truncate(raw_value, _MAX_VALUE_CHARS)

        return ASTSymbol(
            name=name,
            kind=SymbolKind.CONSTANT,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=f"const {name}{type_text} = {value_text}",
            docstring="",
            parent_class="",
            decorators=(),
            bases=(),
            language="rust",
            is_public=self._rust_is_public(node),
            is_async=False,
        )

    def _extract_rust_type_alias(
        self,
        node: Node,
        source: bytes,
        module_path: str,
    ) -> ASTSymbol | None:
        """Extract a type_item (type alias)."""
        name_node = node.child_by_field_name("name")
        if name_node is None:
            return None
        name = self._node_text(name_node, source)
        full_text = _truncate(self._node_text(node, source), _MAX_TYPE_ALIAS_CHARS)

        return ASTSymbol(
            name=name,
            kind=SymbolKind.TYPE_ALIAS,
            module_path=module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=full_text,
            docstring="",
            parent_class="",
            decorators=(),
            bases=(),
            language="rust",
            is_public=self._rust_is_public(node),
            is_async=False,
        )

    def _extract_rust_imports(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        imports: list[ImportStatement],
    ) -> None:
        """Extract use declarations from a Rust AST."""
        _ = module_path
        for child in root.children:
            if child.type == "use_declaration":
                self._extract_rust_use(child, source, imports)

    def _extract_rust_use(
        self,
        node: Node,
        source: bytes,
        imports: list[ImportStatement],
    ) -> None:
        """Extract a single use_declaration as an ImportStatement."""
        for child in node.children:
            if child.type in ("use", ";", "visibility_modifier"):
                continue
            path_text = self._node_text(child, source)
            if path_text:
                imports.append(
                    ImportStatement(
                        module=path_text,
                        names=(),
                        is_relative=path_text.startswith(("self::", "super::", "crate::")),
                        relative_level=0,
                        line_number=node.start_point[0] + 1,
                        resolved_path="",
                    )
                )
            break

    # ------------------------------------------------------------------
    # Python import extraction (unchanged)
    # ------------------------------------------------------------------

    def _extract_python_imports(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        imports: list[ImportStatement],
    ) -> None:
        """Extract import statements from a Python AST."""
        _ = module_path
        for child in root.children:
            if child.type == "import_statement":
                self._extract_bare_import(child, source, imports)
            elif child.type == "import_from_statement":
                self._extract_from_import(child, source, imports)

    def _extract_bare_import(
        self,
        node: Node,
        source: bytes,
        imports: list[ImportStatement],
    ) -> None:
        """Extract 'import foo' or 'import foo, bar' statements."""
        for child in node.children:
            if child.type == "dotted_name":
                module_name = self._node_text(child, source)
                imports.append(
                    ImportStatement(
                        module=module_name,
                        names=(),
                        is_relative=False,
                        relative_level=0,
                        line_number=node.start_point[0] + 1,
                        resolved_path="",
                    )
                )
            elif child.type == "aliased_import":
                name_node = child.child_by_field_name("name")
                if name_node:
                    module_name = self._node_text(name_node, source)
                    imports.append(
                        ImportStatement(
                            module=module_name,
                            names=(),
                            is_relative=False,
                            relative_level=0,
                            line_number=node.start_point[0] + 1,
                            resolved_path="",
                        )
                    )

    def _extract_from_import(
        self,
        node: Node,
        source: bytes,
        imports: list[ImportStatement],
    ) -> None:
        """Extract 'from foo import bar' statements."""
        module_node = node.child_by_field_name("module_name")
        module_name, relative_level = self._resolve_from_module(node, module_node, source)
        names = self._collect_import_names(node, module_node, source)

        imports.append(
            ImportStatement(
                module=module_name,
                names=tuple(names),
                is_relative=relative_level > 0,
                relative_level=relative_level,
                line_number=node.start_point[0] + 1,
                resolved_path="",
            )
        )

    def _resolve_from_module(
        self, node: Node, module_node: Node | None, source: bytes
    ) -> tuple[str, int]:
        """Resolve module name and relative level from a 'from' import node."""
        module_name = ""
        relative_level = 0

        if module_node:
            if module_node.type == "relative_import":
                for prefix_child in module_node.children:
                    if prefix_child.type == "import_prefix":
                        relative_level = len(self._node_text(prefix_child, source))
                    elif prefix_child.type == "dotted_name":
                        module_name = self._node_text(prefix_child, source)
            else:
                module_name = self._node_text(module_node, source)
        else:
            for child in node.children:
                text = self._node_text(child, source)
                if text and all(c == "." for c in text) and child.type not in ("import", "from"):
                    relative_level = len(text)
                    break

        return module_name, relative_level

    def _collect_import_names(
        self, node: Node, module_node: Node | None, source: bytes
    ) -> list[str]:
        """Collect imported names from a 'from' import node."""
        names: list[str] = []
        for child in node.children:
            if child.type == "dotted_name" and child != module_node:
                names.append(self._node_text(child, source))
            elif child.type == "aliased_import":
                name_node = child.child_by_field_name("name")
                if name_node:
                    names.append(self._node_text(name_node, source))
            elif child.type == "wildcard_import":
                names.append("*")
        return names

    # ------------------------------------------------------------------
    # Docstring / decorator helpers (Python-specific)
    # ------------------------------------------------------------------

    def _get_docstring(self, node: Node, source: bytes) -> str:
        """Get first line of docstring from a function or class body.

        Same grammar drift as `_try_extract_constant`: the docstring is either
        the body's first child (pack 1.14.3) or the first child of an
        `expression_statement` wrapper (older revisions).
        """
        body = node.child_by_field_name("body")
        if body is None or not body.children:
            return ""

        first_stmt = body.children[0]
        if first_stmt.type == "string":
            string_node = first_stmt
        elif first_stmt.type == "expression_statement" and first_stmt.children:
            string_node = first_stmt.children[0]
        else:
            return ""

        if string_node.type != "string":
            return ""

        text = self._node_text(string_node, source)
        for q in ('"""', "'''"):
            if text.startswith(q) and text.endswith(q):
                text = text[3:-3]
                break
        else:
            if len(text) >= _MIN_QUOTED_LITERAL_CHARS:
                text = text[1:-1]

        for line in text.strip().split("\n"):
            stripped = line.strip()
            if stripped:
                return stripped
        return ""

    def _get_decorators(self, decorated_node: Node, source: bytes) -> tuple[str, ...]:
        """Get decorator names from a decorated_definition node."""
        decorators = []
        for child in decorated_node.children:
            if child.type == "decorator":
                text = self._node_text(child, source)
                if text.startswith("@"):
                    text = text[1:]
                decorators.append(text)
        return tuple(decorators)

    def _get_own_decorators(self, node: Node, source: bytes) -> tuple[str, ...]:
        """Get decorators if this node's parent is a decorated_definition."""
        if node.parent and node.parent.type == "decorated_definition":
            return self._get_decorators(node.parent, source)
        return ()

    def _get_bases(self, class_node: Node, source: bytes) -> tuple[str, ...]:
        """Get base class names from a class definition."""
        superclasses = class_node.child_by_field_name("superclasses")
        if superclasses is None:
            return ()

        bases = []
        for child in superclasses.children:
            if child.is_named:
                bases.append(self._node_text(child, source))
        return tuple(bases)

    # ------------------------------------------------------------------
    # Shared utilities
    # ------------------------------------------------------------------

    @staticmethod
    def _node_text(node: Node | None, source: bytes) -> str:
        """Get the text content of a node."""
        if node is None:
            return ""
        return source[node.start_byte : node.end_byte].decode("utf-8", errors="replace")

    @staticmethod
    def _strip_quotes(text: str) -> str:
        """Remove surrounding single/double/backtick quotes from a string."""
        text = text.strip()
        for q in ('"""', "'''", '"', "'", "`"):
            if text.startswith(q) and text.endswith(q) and len(text) >= len(q) * 2:
                return text[len(q) : -len(q)]
        return text
