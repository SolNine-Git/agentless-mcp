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
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from functools import partial
from typing import ClassVar

from tree_sitter import Node, Parser

from agentless_mcp.core import grammars
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.symbols import ASTSymbol, Rationale, SymbolKind, disambiguate

# Handlers normalised for the registry: both take the parsed root, the source
# bytes, the module path, and the accumulator list they append to.
SymbolHandler = Callable[[Node, bytes, str, list[ASTSymbol]], None]
ImportHandler = Callable[[Node, bytes, str, list[ImportStatement]], None]

logger = logging.getLogger(__name__)


class UnsupportedLanguageError(ValueError):
    """A language name this extractor's registry does not carry.

    Its own class rather than a bare ``ValueError`` so that the extraction
    entry points can tell "we do not extract this language" -- an ordinary,
    expected outcome that yields no symbols -- apart from a grammar that
    refused to load, which `tree_sitter.Parser` also reports as a
    ``ValueError`` and which is a degradation the caller has to see. Kept a
    ``ValueError`` subclass because that is what `get_parser` has always
    raised for an unknown language.
    """


_UPPER_CASE_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")

# Signatures are an index, not a rendering of the source: long initialisers and
# type aliases are truncated so one pathological literal cannot dominate a view.
_MAX_VALUE_CHARS = 80
_MAX_TYPE_ALIAS_CHARS = 120
# The shortest string literal that still has a quote at each end.
_MIN_QUOTED_LITERAL_CHARS = 2

# Rationale comments are structural source facts, not generated summaries.
# Only comment nodes reach these expressions, so a string containing
# ``TODO:`` is never promoted into the graph. Text is capped before it enters
# the cache or any response.
_RATIONALE_MARKER = re.compile(r"\b(NOTE|WHY|HACK|TODO)\s*:\s*(.*)")
_RATIONALE_CITATION = re.compile(r"\b(?:ADR|RFC)(?:[-\s#:]*)(\d+)\b", re.IGNORECASE)
_MAX_RATIONALE_CHARS = 240
_QUOTED_TEXT_MIN_CHARS = 2

# Every grammar in the table spells a plain name `identifier`; the languages
# with richer name node types override this in their LanguageConfig entry.
DEFAULT_IDENTIFIER_NODE_TYPES: tuple[str, ...] = ("identifier",)

# The child node types a declaration's name is looked for in when the
# grammar's `name` field does not resolve.
DEFAULT_NAME_NODE_TYPES: tuple[str, ...] = (
    "identifier",
    "type_identifier",
    "property_identifier",
    "word",
)

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

# Comment node types, for the passes that must ignore comments rather than
# read them -- patch normalisation in `core.normalize` above all, where a
# comment-only edit has to hash to the same key as no edit at all. One flat
# set rather than a per-language column: verified 2026-08-18 by probe-parsing
# every tier-1 grammar, python/go/c/cpp/lua/ruby/bash and the ECMAScript
# family spell it `comment` while rust and java split it into `line_comment`
# and `block_comment`, and no grammar in the table gives any of those three
# names to something that is not a comment. Nested comment content (lua's
# `comment_content`) needs no entry: consumers skip a comment's whole subtree.
COMMENT_NODE_TYPES: frozenset[str] = frozenset({"comment", "line_comment", "block_comment"})

# Block node types whose delimiters are INVISIBLE in a token stream. They live
# beside the identifier and comment tables because they are the same kind of
# fact -- what a node type means in one grammar -- and a language added to one
# table needs considering for the others.
#
# `core.normalize` emits an explicit marker around each of these. Every other
# tier-1 language closes a block with a token that is in the stream already:
# `}` in the braced languages, `end` in ruby and lua, `fi`/`done` in bash.
# Python closes one with a DEDENT that tree-sitter keeps hidden, so without a
# marker a patch that dedents a statement out of an `if` produces a byte-
# identical stream to one that does not. Measured 2026-08-18: identical keys
# without the marker, distinct keys with it.
INDENT_BLOCK_NODE_TYPES: dict[str, tuple[str, ...]] = {
    "python": ("block",),
}

# Node types that ARE a statement block: the body of a function, whatever the
# language calls it. One flat set for the same reason as the comment table --
# a node type means the same thing wherever it appears, and no grammar in the
# table gives one of these names to something that is not a block. Read by
# `core.skeleton` to decide what to elide and here to decide where a
# declaration's header ends.
BODY_BLOCK_NODE_TYPES: frozenset[str] = frozenset(
    {
        "block",
        "statement_block",
        "compound_statement",
        "body_statement",
        "do_block",
        "function_body",
    }
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

    The four fields below `identifier_node_types` exist because not every
    grammar in the pack names its fields.  tree-sitter-kotlin exposes no
    `name`, `parameters` or `body` field at all, and tree-sitter-swift gives
    the return type the same field name as the function name, so a table that
    could only address fields would silently extract nothing for them.  Each
    field is a fallback consulted only when the field lookup finds nothing, so
    a language that does name its fields is unaffected.
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
    # Child node types that carry a declaration's name when `name_field` does
    # not resolve.
    name_node_types: tuple[str, ...] = DEFAULT_NAME_NODE_TYPES
    # Child node types holding a class's members when the class node has no
    # `body` field (kotlin: `class_body`).
    class_body_node_types: tuple[str, ...] = ()
    # Render a function's signature from its own header text -- everything
    # from the declaration's first byte to the start of its body -- instead of
    # from `fn <name>(<parameters>)`. For grammars with no `parameters` field,
    # where the composed form would claim every function takes none.
    signature_from_header: bool = False
    # Field on a function node naming the type the function is a method of,
    # for languages that declare methods outside the type's own body. Go is
    # the case in the table: `func (c *Config) Validate()` is a top-level
    # declaration, so without the receiver every `Validate` in a file is the
    # same qualified name and the same stable id. The field is the grammar's
    # own, so the owner is read rather than guessed.
    receiver_field: str | None = None
    # Node types inside an import statement that hold the names it binds into
    # the importing file's own namespace. Empty for most languages, and the
    # emptiness is a claim rather than an omission: `import "fmt"` in Go and
    # `import a.b` in Python bind a module object, not the module's contents,
    # so a bare reference to a name defined in that module is not evidence of
    # an import. The ECMAScript family is the case in the table, where
    # `import { X } from "./m"` genuinely does bind `X`.
    #
    # `core/resolve` depends on this being right. While the names were dropped,
    # the resolver could only reach a TypeScript named import through its
    # whole-module set -- which is why that set was allowed to supply
    # bare-name evidence for every language, including the ones where it means
    # nothing of the kind.
    import_name_node_types: tuple[str, ...] = ()
    # Child node types carrying a declaration's modifier keywords. Two shapes
    # in the pack -- a container whose children are the keywords (java,
    # kotlin, scala `modifiers`) and one leaf per keyword (csharp `modifier`,
    # php `visibility_modifier`) -- so the keywords are read as text and split
    # rather than by node type. Empty means the language has no visibility
    # keywords and the leading-underscore convention is the only signal there
    # is.
    modifier_node_types: tuple[str, ...] = ()
    # Node types that bind a name to a value, where a function-valued binding
    # is how the language declares a function. `export const App = () => {}`
    # is the dominant form in modern ECMAScript, and matching only
    # `function_declaration` reports a React component file as empty.
    binding_node_types: tuple[str, ...] = ()
    # Value node types that make such a binding a function declaration.
    function_value_node_types: tuple[str, ...] = ()
    # Node types that declare a constant, plus every modifier keyword the
    # declaration must carry to be one. Java needs both: `static final double
    # TAX_RATE` is a constant of the class and `private final double subtotal`
    # is per-instance state that happens not to be reassigned. Reporting the
    # second as a constant would make the map claim a guarantee the code does
    # not give.
    constant_node_types: tuple[str, ...] = ()
    constant_modifier_keywords: tuple[str, ...] = ()


# Modifier keywords that deny a declaration to importers. `internal` is C#
# and Kotlin's assembly/module scope and `fileprivate` is Swift's; both are
# narrower than public, which is the question `is_public` answers.
NON_PUBLIC_MODIFIERS = frozenset({"private", "protected", "internal", "fileprivate"})

# The ECMAScript value forms that make `const name = <value>` a function
# declaration. Shared by the three grammars in the family.
_ECMASCRIPT_FUNCTION_VALUES = (
    "arrow_function",
    "function_expression",
    "function",
    "generator_function",
)


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
        import_name_node_types=("import_specifier", "import_clause"),
        binding_node_types=("variable_declarator",),
        function_value_node_types=_ECMASCRIPT_FUNCTION_VALUES,
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
        import_name_node_types=("import_specifier", "import_clause"),
        binding_node_types=("variable_declarator",),
        function_value_node_types=_ECMASCRIPT_FUNCTION_VALUES,
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
        import_name_node_types=("import_specifier", "import_clause"),
        binding_node_types=("variable_declarator",),
        function_value_node_types=_ECMASCRIPT_FUNCTION_VALUES,
    ),
    "go": LanguageConfig(
        function_node_types=("function_declaration", "method_declaration"),
        # `type_spec`, not `type_declaration`: the outer node names nothing,
        # so `type Invoice struct` used to match and yield no symbol at all --
        # leaving every method's receiver type with no declaration of its own.
        class_node_types=("type_spec",),
        import_node_types=("import_spec",),
        name_field="name",
        import_path_node_type="interpreted_string_literal",
        identifier_node_types=(
            "identifier",
            "type_identifier",
            "field_identifier",
            "package_identifier",
        ),
        receiver_field="receiver",
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
        modifier_node_types=("modifiers",),
        constant_node_types=("field_declaration",),
        constant_modifier_keywords=("static", "final"),
    ),
    "ruby": LanguageConfig(
        function_node_types=("method", "singleton_method"),
        class_node_types=("class", "module"),
        import_node_types=(),  # require calls detected separately
        name_field="name",
        identifier_node_types=("identifier", "constant"),
    ),
    # -- tier 2 (2026-08-18, Phase 4): node types read off the pack's own
    # parse trees rather than from documentation, one probe per language.
    "php": LanguageConfig(
        function_node_types=("function_definition", "method_declaration"),
        class_node_types=(
            "class_declaration",
            "interface_declaration",
            "trait_declaration",
            "enum_declaration",
        ),
        import_node_types=("namespace_use_declaration",),
        name_field="name",
        # `use App\Money\Currency;` nests the path one level down, in the
        # `qualified_name` inside the use clause.
        import_path_node_type="qualified_name",
        # A php `name` node is the leaf under every identifier, `$var`
        # included: `variable_name` wraps `$` and a `name`, and the reference
        # pass only reads leaves.
        identifier_node_types=("name",),
        modifier_node_types=("visibility_modifier",),
    ),
    "kotlin": LanguageConfig(
        function_node_types=("function_declaration",),
        class_node_types=("class_declaration", "object_declaration"),
        import_node_types=("import_header",),
        # tree-sitter-kotlin names no fields at all, so every lookup below is
        # the child-type fallback.
        name_field=None,
        import_path_node_type="identifier",
        identifier_node_types=("simple_identifier", "type_identifier"),
        name_node_types=("simple_identifier", "type_identifier"),
        class_body_node_types=("class_body",),
        signature_from_header=True,
        modifier_node_types=("modifiers",),
    ),
    "swift": LanguageConfig(
        function_node_types=(
            "function_declaration",
            "protocol_function_declaration",
            "init_declaration",
        ),
        class_node_types=("class_declaration", "protocol_declaration"),
        import_node_types=("import_declaration",),
        name_field="name",
        import_path_node_type="identifier",
        identifier_node_types=("simple_identifier", "type_identifier"),
        # Parameters are direct children of the declaration with no wrapper
        # node, and the return type reuses the `name` field, so a composed
        # signature would read `fn applyTax()` for a two-parameter function.
        signature_from_header=True,
    ),
    "scala": LanguageConfig(
        function_node_types=("function_definition", "function_declaration"),
        class_node_types=(
            "class_definition",
            "trait_definition",
            "object_definition",
            "enum_definition",
        ),
        import_node_types=("import_declaration",),
        import_path_node_type="identifier",
        identifier_node_types=("identifier", "type_identifier"),
        class_body_node_types=("template_body",),
        signature_from_header=True,
        modifier_node_types=("modifiers",),
    ),
    "csharp": LanguageConfig(
        function_node_types=(
            "method_declaration",
            "constructor_declaration",
            "local_function_statement",
        ),
        class_node_types=(
            "class_declaration",
            "interface_declaration",
            "struct_declaration",
            "record_declaration",
            "enum_declaration",
        ),
        import_node_types=("using_directive",),
        import_path_node_type="identifier",
        identifier_node_types=("identifier",),
        class_body_node_types=("declaration_list",),
        signature_from_header=True,
        modifier_node_types=("modifier",),
    ),
    # Deterministic non-code surfaces use dedicated symbol handlers below;
    # these rows own their reference node types and keep the registry's trust
    # metadata in the same table as every other tier-2 language.
    "json": LanguageConfig((), (), (), identifier_node_types=("string_content",)),
    "toml": LanguageConfig((), (), (), identifier_node_types=("bare_key", "quoted_key")),
    "yaml": LanguageConfig((), (), (), identifier_node_types=("string_scalar",)),
    "hcl": LanguageConfig((), (), (), identifier_node_types=("identifier", "template_literal")),
    "sql": LanguageConfig((), (), (), identifier_node_types=("identifier",)),
}

# C and C++ are handled by dedicated methods due to their nested declarator
# structure (function name lives in declarator.declarator, not a direct field).

# Identifier node types for the languages whose extraction is done by a
# dedicated handler, so they have no LanguageConfig entry to carry them.
_EXTRA_IDENTIFIER_NODE_TYPES: dict[str, tuple[str, ...]] = {
    "python": ("identifier",),
    "rust": ("identifier", "type_identifier", "field_identifier"),
    "c": ("identifier", "type_identifier", "field_identifier"),
    "cpp": ("identifier", "type_identifier", "field_identifier", "namespace_identifier"),
}


class IdentifierRole(str, Enum):
    """The syntactic meaning of one identifier occurrence."""

    REFERENCE = "reference"
    LOCAL = "local"
    BINDING = "binding"
    DECLARATION = "declaration"
    IMPORT = "import"
    ATTRIBUTE = "attribute"
    MODULE_QUALIFIER = "module_qualifier"
    MODULE_ATTRIBUTE = "module_attribute"
    KEYWORD = "keyword"


@dataclass(frozen=True)
class Ref:
    """One identifier occurrence: which file spelled which name, and where.

    Lives beside the node-type table rather than in `core.refs` because the
    reference pass is a parse like any other and because `core.cache` stores
    these rows -- and a cache that imported the scanner would invert the
    dependency between them.

    ``role`` records whether the occurrence is a reference at all. Keeping
    every occurrence lets literal fan-in list sites without allowing an
    assignment target, keyword label, or attribute member to fabricate a
    structural relationship.
    """

    path: str
    name: str
    line: int
    role: IdentifierRole = IdentifierRole.REFERENCE
    qualifier: str = ""

    @property
    def is_reference(self) -> bool:
        """Whether this is an unqualified repository-symbol reference."""
        return self.role is IdentifierRole.REFERENCE

    @property
    def is_resolvable(self) -> bool:
        """Whether the evidence graph has enough syntax to attempt binding."""
        return self.role in {IdentifierRole.REFERENCE, IdentifierRole.MODULE_ATTRIBUTE}


# Python nodes that open a lexical scope, with the field naming parameters
# where one exists. Comprehensions have their own target bindings; class scope
# is deliberately skipped when resolving names inside a nested method because
# Python methods do not close over their class namespace.
_PYTHON_SCOPE_FIELDS: dict[str, str] = {
    "function_definition": "parameters",
    "lambda": "parameters",
}
_PYTHON_SCOPE_TYPES: dict[str, str] = {
    "function_definition": "function",
    "lambda": "function",
    "class_definition": "class",
    "list_comprehension": "comprehension",
    "set_comprehension": "comprehension",
    "dictionary_comprehension": "comprehension",
    "generator_expression": "comprehension",
}

# Parameter forms whose name sits behind a `name` field, with annotation or
# default expressions as siblings.
_PYTHON_NAMED_PARAMETERS = ("default_parameter", "typed_default_parameter")

# `*args` / `**kwargs`: the bound name is the wrapped identifier.
_PYTHON_SPLAT_PATTERNS = ("list_splat_pattern", "dictionary_splat_pattern")


@dataclass(frozen=True)
class _BindingScope:
    """One Python lexical scope and the names that cannot bind elsewhere."""

    start_byte: int
    end_byte: int
    kind: str
    bindings: frozenset[str]
    globals: frozenset[str]
    imports: frozenset[str]


@dataclass
class _ScopeBuilder:
    """Mutable collection state used only while one Python tree is classified."""

    node_id: int
    start_byte: int
    end_byte: int
    kind: str
    bindings: set[str]
    globals: set[str]
    imports: set[str]


def _python_parameter_nodes(params: Node) -> tuple[Node, ...]:
    """Extract the identifier nodes a Python parameter list binds.

    Annotation and default expressions are deliberately left out: `counter:
    TokenCounter = DEFAULT` binds `counter`, while `TokenCounter` and
    `DEFAULT` are ordinary references the resolver must still see.
    """
    nodes: list[Node] = []
    for child in params.named_children:
        target: Node | None = child
        if child.type in _PYTHON_NAMED_PARAMETERS:
            target = child.child_by_field_name("name")
        if target is not None and target.type == "typed_parameter":
            annotation = target.child_by_field_name("type")
            annotation_id = annotation.id if annotation is not None else None
            target = next(
                (part for part in target.named_children if part.id != annotation_id),
                None,
            )
        if target is not None and target.type in _PYTHON_SPLAT_PATTERNS:
            target = next(iter(target.named_children), None)
        if target is not None and target.type == "identifier":
            nodes.append(target)
    return tuple(nodes)


def _python_roles(
    root: Node,
    data: bytes,
) -> tuple[dict[int, IdentifierRole], dict[int, str], tuple[_BindingScope, ...]]:
    """Classify Python identifiers and collect lexical bindings in two passes."""
    nodes = walk_nodes(root)
    builders = [_scope_builder(root, "module")]
    builders.extend(
        _scope_builder(node, kind)
        for node in nodes
        if (kind := _PYTHON_SCOPE_TYPES.get(node.type)) is not None
    )
    by_node = {scope.node_id: scope for scope in builders}
    roles: dict[int, IdentifierRole] = {}
    qualifiers: dict[int, str] = {}

    for node in nodes:
        if node.type in {"import_statement", "import_from_statement"}:
            for identifier in _descendant_identifiers(node):
                roles[identifier.id] = IdentifierRole.IMPORT
            _nearest_scope(node, builders).imports.update(_imported_module_names(node, data))

    for node in nodes:
        _bind_python_declaration(node, builders, data)

        field = _PYTHON_SCOPE_FIELDS.get(node.type)
        if field is not None:
            params = node.child_by_field_name(field)
            if params is not None:
                bound = _python_parameter_nodes(params)
                _mark_bindings(bound, roles)
                scope = by_node[node.id]
                scope.bindings.update(_node_text(identifier, data) for identifier in bound)

        target = _binding_target(node)
        if target is not None:
            bound = _binding_identifiers(target)
            _mark_bindings(bound, roles)
            scope = _nearest_scope(node, builders)
            names = {_node_text(identifier, data) for identifier in bound}
            if scope.kind == "module":
                names = {name for name in names if _UPPER_CASE_RE.fullmatch(name) is None}
            scope.bindings.update(names)

        if node.type in {"global_statement", "nonlocal_statement"}:
            declarations = tuple(_descendant_identifiers(node))
            _mark_bindings(declarations, roles)
            scope = _nearest_scope(node, builders)
            names = {_node_text(identifier, data) for identifier in declarations}
            if node.type == "global_statement":
                scope.globals.update(names)
            else:
                scope.bindings.update(names)

    for node in nodes:
        _mark_non_reference_roles(node, roles, qualifiers, builders, data)

    scopes = tuple(
        sorted(
            (
                _BindingScope(
                    start_byte=scope.start_byte,
                    end_byte=scope.end_byte,
                    kind=scope.kind,
                    bindings=frozenset(scope.bindings),
                    globals=frozenset(scope.globals),
                    imports=frozenset(scope.imports),
                )
                for scope in builders
            ),
            key=lambda scope: (scope.end_byte - scope.start_byte, scope.start_byte),
        )
    )
    return roles, qualifiers, scopes


def _bind_python_declaration(node: Node, scopes: Sequence[_ScopeBuilder], data: bytes) -> None:
    """Bind a nested function or class name in its enclosing lexical scope."""
    if node.type not in {"function_definition", "class_definition"}:
        return
    name = node.child_by_field_name("name")
    if name is None:
        return
    scope = _nearest_scope(name, scopes)
    if scope.kind != "module":
        scope.bindings.add(_node_text(name, data))


def _scope_builder(node: Node, kind: str) -> _ScopeBuilder:
    """Build the byte boundary where one scope's bindings apply."""
    body = node.child_by_field_name("body") if kind in {"function", "class"} else None
    boundary = body if body is not None else node
    return _ScopeBuilder(
        node_id=node.id,
        start_byte=boundary.start_byte,
        end_byte=boundary.end_byte,
        kind=kind,
        bindings=set(),
        globals=set(),
        imports=set(),
    )


def _nearest_scope(node: Node, scopes: Sequence[_ScopeBuilder]) -> _ScopeBuilder:
    """Return the innermost lexical scope containing ``node``."""
    candidates = [
        scope
        for scope in scopes
        if scope.start_byte <= node.start_byte and node.end_byte <= scope.end_byte
    ]
    return min(candidates, key=lambda scope: scope.end_byte - scope.start_byte)


def _mark_non_reference_roles(
    node: Node,
    roles: dict[int, IdentifierRole],
    qualifiers: dict[int, str],
    scopes: Sequence[_ScopeBuilder],
    data: bytes,
) -> None:
    """Mark declaration, member, and label identifiers from named fields."""
    if node.type == "attribute":
        attribute = node.child_by_field_name("attribute")
        if attribute is not None:
            object_node = node.child_by_field_name("object")
            qualifier = (
                _node_text(object_node, data)
                if object_node is not None and object_node.type == "identifier"
                else None
            )
            imported = qualifier is not None and _binds_imported_module(qualifier, node, scopes)
            role = IdentifierRole.MODULE_ATTRIBUTE if imported else IdentifierRole.ATTRIBUTE
            roles.setdefault(attribute.id, role)
            if imported and object_node is not None and qualifier is not None:
                roles.setdefault(object_node.id, IdentifierRole.MODULE_QUALIFIER)
                # The name the source spells, which is what `core/resolve`
                # keys a module binding on. `import a.b as ab` is written
                # `ab.f()` here, and `a` is a name this file never binds.
                qualifiers[attribute.id] = qualifier
    if node.type == "keyword_argument":
        name = node.child_by_field_name("name")
        if name is not None:
            roles.setdefault(name.id, IdentifierRole.KEYWORD)
    if node.type in {"function_definition", "class_definition"}:
        name = node.child_by_field_name("name")
        if name is not None:
            roles.setdefault(name.id, IdentifierRole.DECLARATION)


def _imported_module_names(node: Node, data: bytes) -> set[str]:
    """Return the local names one import statement introduces.

    Names, not a mapping to the module behind them: the only consumer is the
    qualifier a module attribute carries, and a reference spells the local
    name. `import a.b as ab` introduces `ab`, `import a.b` introduces `a`, and
    `from pkg import mod` introduces `mod`.
    """
    names: set[str] = set()
    for index, child in enumerate(node.children):
        if node.field_name_for_child(index) != "name":
            continue
        alias = child.child_by_field_name("alias")
        if alias is not None:
            names.add(_node_text(alias, data))
            continue
        identifiers = _descendant_identifiers(child)
        if not identifiers:
            continue
        # `import a.b` binds the package `a`; `from pkg import mod` binds
        # `mod`, the last segment.
        selected = identifiers[0] if node.type == "import_statement" else identifiers[-1]
        names.add(_node_text(selected, data))
    return names


def _binds_imported_module(name: str, node: Node, scopes: Sequence[_ScopeBuilder]) -> bool:
    """True when ``name`` is an imported module object visible at ``node``."""
    candidates = sorted(
        (
            scope
            for scope in scopes
            if scope.start_byte <= node.start_byte and node.end_byte <= scope.end_byte
        ),
        key=lambda scope: scope.end_byte - scope.start_byte,
    )
    inside_function = False
    for scope in candidates:
        if scope.kind == "function":
            inside_function = True
        if scope.kind == "class" and inside_function:
            continue
        if name in scope.globals:
            continue
        if name in scope.bindings:
            return False
        if name in scope.imports:
            return True
    return False


def _binding_target(node: Node) -> Node | None:
    """Return the subtree a binding construct assigns, when it has one."""
    fields = {
        "assignment": "left",
        "augmented_assignment": "left",
        "named_expression": "name",
        "for_statement": "left",
        "for_in_clause": "left",
        "as_pattern": "alias",
    }
    field = fields.get(node.type)
    return node.child_by_field_name(field) if field is not None else None


def _binding_identifiers(target: Node) -> tuple[Node, ...]:
    """Return names actually bound by a target, excluding member/subscript parts."""
    if target.type in {"attribute", "subscript"}:
        return ()
    if target.type == "identifier":
        return (target,)
    return tuple(
        identifier for child in target.named_children for identifier in _binding_identifiers(child)
    )


def _descendant_identifiers(node: Node) -> tuple[Node, ...]:
    """Return every Python identifier below ``node`` in source order."""
    return tuple(entry for entry in walk_nodes(node) if entry.type == "identifier")


def _mark_bindings(nodes: Sequence[Node], roles: dict[int, IdentifierRole]) -> None:
    for node in nodes:
        roles.setdefault(node.id, IdentifierRole.BINDING)


def _node_text(node: Node, data: bytes) -> str:
    return data[node.start_byte : node.end_byte].decode("utf-8", errors="replace")


def _python_role(
    node: Node,
    name: str,
    explicit: dict[int, IdentifierRole],
    scopes: Sequence[_BindingScope],
) -> IdentifierRole:
    """Return the explicit syntactic role or the nearest lexical binding."""
    direct = explicit.get(node.id)
    if direct is not None:
        return direct

    inside_function = False
    for scope in scopes:
        if not (scope.start_byte <= node.start_byte and node.end_byte <= scope.end_byte):
            continue
        if scope.kind == "function":
            inside_function = True
        if scope.kind == "class" and inside_function:
            continue
        if name in scope.globals:
            return IdentifierRole.REFERENCE
        if name in scope.bindings:
            return IdentifierRole.LOCAL
    return IdentifierRole.REFERENCE


def identifier_node_types(language: str) -> frozenset[str]:
    """Return the leaf node types that name something in ``language``."""
    config = LANGUAGE_CONFIGS.get(language)
    if config is not None:
        return frozenset(config.identifier_node_types)
    return frozenset(_EXTRA_IDENTIFIER_NODE_TYPES.get(language, DEFAULT_IDENTIFIER_NODE_TYPES))


def collect_refs(source: str, language: str, path: str) -> list[Ref]:
    """Return every identifier occurrence in ``source``.

    Raises :class:`~agentless_mcp.util.errors.LanguageUnavailable` when the
    grammar is not warmed: a reference pass that quietly returned nothing
    would read as "this symbol is unused", which is the most expensive wrong
    answer this tool could give.
    """
    wanted = identifier_node_types(language)
    if not wanted:
        return []

    parser = grammars.get_parser(language)
    data = source.encode("utf-8")
    tree = parser.parse(data)
    explicit: dict[int, IdentifierRole] = {}
    qualifiers: dict[int, str] = {}
    scopes: tuple[_BindingScope, ...] = ()
    if language == "python":
        explicit, qualifiers, scopes = _python_roles(tree.root_node, data)

    refs: list[Ref] = []
    for node in walk_nodes(tree.root_node):
        if node.type not in wanted or node.child_count:
            continue
        name = _node_text(node, data)
        if name:
            line = node.start_point[0] + 1
            refs.append(
                Ref(
                    path=path,
                    name=name,
                    line=line,
                    role=(
                        _python_role(node, name, explicit, scopes)
                        if language == "python"
                        else IdentifierRole.REFERENCE
                    ),
                    qualifier=qualifiers.get(node.id, ""),
                )
            )
    return refs


def walk_nodes(root: Node) -> list[Node]:
    """Return every node in the tree, parents before children.

    Iterative rather than recursive: a deeply nested expression in a generated
    file must not turn a repository map into a RecursionError.
    """
    found: list[Node] = []
    stack: list[Node] = [root]
    while stack:
        node = stack.pop()
        found.append(node)
        stack.extend(reversed(node.children))
    return found


def declarations_under(root: Node, declaration_types: frozenset[str]) -> Iterator[Node]:
    """Yield each declaration beneath ``root``, descending through wrappers.

    One traversal rule, stated once and shared by every handler that has a set
    of declaration node types: a node that is not itself a declaration is a
    wrapper that may still hold one, and a declaration ends the descent
    because what it contains belongs to it. Include guards, C++ namespaces,
    Rust `mod` bodies and `if TYPE_CHECKING:` blocks are all the same shape,
    and a loop over the root's direct children reaches inside none of them.

    Iterative for the reason :func:`walk_nodes` gives: a deep chain of nodes
    must not exhaust the interpreter's stack and abort a repository index.
    """
    stack: list[Node] = list(reversed(root.children))
    while stack:
        node = stack.pop()
        if node.type in declaration_types:
            yield node
            continue
        stack.extend(reversed(node.children))


def declares_async(node: Node) -> bool:
    """True when a declaration node carries an `async` keyword of its own.

    Two grammars spell it differently and the question is one: Python puts
    `async` among the `function_definition`'s own children, and Rust nests it
    inside a `function_modifiers` node beside the visibility modifier.

    Asked of the tree rather than of the source text. A substring search over
    a declaration's first bytes reports `def async_handler(...)` as async, and
    `is_async` is persisted to the tag cache and rendered into the signature
    an agent reads -- so telling it to await a synchronous function is an
    instruction, not a display nit.
    """
    for child in node.children:
        if child.type == "async":
            return True
        if child.type == "function_modifiers" and any(
            modifier.type == "async" for modifier in child.children
        ):
            return True
    return False


def _extract_rationales(root: Node, source: bytes) -> tuple[Rationale, ...]:
    """Extract rationale markers and ADR/RFC citations from comment nodes."""
    found: list[Rationale] = []
    for node in walk_nodes(root):
        if node.type not in COMMENT_NODE_TYPES:
            continue
        comment = source[node.start_byte : node.end_byte].decode("utf-8", errors="replace")
        for offset, raw_line in enumerate(comment.splitlines()):
            line = _comment_text(raw_line)
            marker = _RATIONALE_MARKER.search(line)
            citations = tuple(
                match.group(0).strip() for match in _RATIONALE_CITATION.finditer(line)
            )
            if marker is None and not citations:
                continue
            kind = marker.group(1).lower() if marker is not None else "citation"
            text = marker.group(2).strip() if marker is not None else line
            if not text:
                text = ", ".join(citations)
            found.append(
                Rationale(
                    kind=kind,
                    text=_truncate(text, _MAX_RATIONALE_CHARS),
                    line_number=node.start_point[0] + offset + 1,
                    citations=citations,
                )
            )
    return tuple(found)


def _comment_text(raw: str) -> str:
    """Remove common comment delimiters from one tree-sitter comment line."""
    text = raw.strip()
    for prefix in ("<!--", "//", "/*", "#", "*"):
        if text.startswith(prefix):
            text = text[len(prefix) :].lstrip()
            break
    for suffix in ("-->", "*/"):
        if text.endswith(suffix):
            text = text[: -len(suffix)].rstrip()
            break
    return text


def _attach_rationales(
    symbols: list[ASTSymbol], rationales: tuple[Rationale, ...]
) -> list[ASTSymbol]:
    """Attach each rationale to the innermost symbol whose span contains it."""
    attached: dict[int, list[Rationale]] = {}
    for rationale in rationales:
        containing = [
            (position, symbol)
            for position, symbol in enumerate(symbols)
            if symbol.line_number
            <= rationale.line_number
            <= (symbol.end_line_number or symbol.line_number)
        ]
        if not containing:
            continue
        position, _ = max(containing, key=lambda pair: pair[1].line_number)
        siblings = attached.setdefault(position, [])
        duplicate_index = sum(
            1 for existing in siblings if existing.line_number == rationale.line_number
        )
        siblings.append(replace(rationale, duplicate_index=duplicate_index))

    return [
        replace(symbol, rationales=tuple(attached[position])) if position in attached else symbol
        for position, symbol in enumerate(symbols)
    ]


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
class _FunctionSite:
    """The three parts of a function symbol that vary by where it was found.

    ``header`` is the node the signature is rendered from. It is the
    declaration itself except for a function-valued binding, where the name is
    on the binding and the parameters are on the value.
    """

    name: str
    owner: str
    header: Node


@dataclass(frozen=True)
class _SurfaceDeclaration:
    """One parsed non-code declaration before common symbol fields are added."""

    name: str
    kind: SymbolKind
    node: Node
    signature: str
    parent: str = ""


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


# The node types each dedicated handler treats as a declaration. They are
# what `declarations_under` stops the descent at, so a type listed here also
# says "what is inside this belongs to it" -- which is why a C++
# `class_specifier` is listed and a `namespace_definition` is not.
C_TAGGED_TYPES = frozenset({"struct_specifier", "class_specifier", "enum_specifier"})
C_DECLARATION_TYPES = frozenset({"function_definition"}) | C_TAGGED_TYPES
RUST_ITEM_TYPES = frozenset(
    {
        "function_item",
        "struct_item",
        "enum_item",
        "trait_item",
        "impl_item",
        "const_item",
        "type_item",
    }
)
PYTHON_DECLARATION_TYPES = frozenset(
    {
        "function_definition",
        "class_definition",
        "expression_statement",
        "assignment",
        "type_alias_statement",
        "decorated_definition",
    }
)


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
        # Tier 2 (generic extractor)
        ".php": "php",
        ".kt": "kotlin",
        ".kts": "kotlin",
        ".swift": "swift",
        ".scala": "scala",
        ".cs": "csharp",
        # Deterministic non-code surfaces (tier 2).
        ".json": "json",
        ".toml": "toml",
        ".yaml": "yaml",
        ".yml": "yaml",
        ".tf": "hcl",
        ".hcl": "hcl",
        ".sql": "sql",
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
            "php": _LanguageSpec("php", generic_symbols("php"), generic_imports("php")),
            "kotlin": _LanguageSpec("kotlin", generic_symbols("kotlin"), generic_imports("kotlin")),
            "swift": _LanguageSpec("swift", generic_symbols("swift"), generic_imports("swift")),
            "scala": _LanguageSpec("scala", generic_symbols("scala"), generic_imports("scala")),
            "csharp": _LanguageSpec("csharp", generic_symbols("csharp"), generic_imports("csharp")),
            "json": _LanguageSpec(
                "json", partial(self._extract_data_symbols, language="json"), self._no_imports
            ),
            "toml": _LanguageSpec(
                "toml", partial(self._extract_data_symbols, language="toml"), self._no_imports
            ),
            "yaml": _LanguageSpec(
                "yaml", partial(self._extract_data_symbols, language="yaml"), self._no_imports
            ),
            "hcl": _LanguageSpec("hcl", self._extract_hcl_symbols, self._no_imports),
            "sql": _LanguageSpec("sql", self._extract_sql_symbols, self._no_imports),
        }

    # ------------------------------------------------------------------
    # Parser caching
    # ------------------------------------------------------------------

    def get_parser(self, language: str) -> Parser:
        """Return the memoized parser for a supported language."""
        return grammars.get_parser(self._grammar_of(language))

    def _grammar_of(self, language: str) -> str:
        """Map a supported language name onto its grammar name."""
        spec = self._registry.get(language)
        if spec is None:
            msg = f"Unsupported language: {language}"
            raise UnsupportedLanguageError(msg)
        return spec.grammar

    # ------------------------------------------------------------------
    # Public symbol extraction API
    # ------------------------------------------------------------------

    def extract_from_source(self, source: str, language: str, module_path: str) -> list[ASTSymbol]:
        """Extract symbols from source string.

        An unsupported language yields no symbols.  A supported language whose
        grammar is unavailable raises `LanguageUnavailable` from
        `core.grammars`, and one whose grammar will not load raises whatever
        the loader raised: both are degradations the caller has to see, not an
        empty file.

        Every handler's output passes through
        `core.symbols.disambiguate` before it leaves this method, so
        stable-id uniqueness inside a file is a property of extraction rather
        than something each of the six handlers has to remember.
        """
        try:
            parser = self.get_parser(language)
        except UnsupportedLanguageError as e:
            logger.warning("Unsupported language %s (%s): %s", language, module_path, e)
            return []

        tree = parser.parse(bytes(source, "utf-8"))
        source_bytes = bytes(source, "utf-8")

        # get_parser succeeded, so the language is registered.
        symbols: list[ASTSymbol] = []
        self._registry[language].extract_symbols(tree.root_node, source_bytes, module_path, symbols)
        defined = disambiguate(symbols)
        rationales = _extract_rationales(tree.root_node, source_bytes)
        return _attach_rationales(defined, rationales)

    # ------------------------------------------------------------------
    # Public import extraction API
    # ------------------------------------------------------------------

    def extract_imports_from_source(
        self, source: str, language: str, module_path: str
    ) -> list[ImportStatement]:
        """Extract import statements from source string.

        Same contract as `extract_from_source`: unsupported means empty, and
        a grammar that is unavailable or will not load raises.
        """
        try:
            parser = self.get_parser(language)
        except UnsupportedLanguageError as e:
            logger.warning("Unsupported language %s (%s): %s", language, module_path, e)
            return []

        tree = parser.parse(bytes(source, "utf-8"))
        source_bytes = bytes(source, "utf-8")

        # get_parser succeeded, so the language is registered.
        imports: list[ImportStatement] = []
        self._registry[language].extract_imports(tree.root_node, source_bytes, module_path, imports)
        return imports

    # ------------------------------------------------------------------
    # Public reference extraction API
    # ------------------------------------------------------------------

    def extract_refs_from_source(self, source: str, language: str, path: str) -> list[Ref]:
        """Extract every identifier occurrence from source string.

        The third of the three parses a repository scan performs, exposed on
        the extractor so that the tag cache -- which stores all three -- has
        one object to ask.
        """
        return collect_refs(source, language, path)

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
        """Visit every node beneath ``node``, parents before children.

        An explicit stack rather than recursion, for the reason `walk_nodes`
        gives: a chain of a few hundred method calls -- routine in a minified
        bundle or a generated client -- is enough to exhaust the interpreter's
        stack, and one such file must not abort the repository index.
        """
        stack: list[tuple[Node, str]] = [(child, parent) for child in reversed(node.children)]
        while stack:
            current, owner = stack.pop()
            stack.extend(reversed(self._visit_generic_node(current, source, walk, owner)))

    def _visit_generic_node(
        self,
        node: Node,
        source: bytes,
        walk: _GenericWalk,
        parent: str,
    ) -> list[tuple[Node, str]]:
        """Emit a symbol if ``node`` matches, and say where the walk goes next.

        The return value is what keeps the traversal iterative: rather than
        descending itself, a visit hands back the ``(node, parent)`` pairs
        still to visit -- a class body under its own name, a wrapper's
        children under the parent they arrived with, and nothing at all under
        a matched function.
        """
        cfg = walk.cfg
        language = walk.language
        module_path = walk.module_path
        symbols = walk.symbols
        if node.type in cfg.function_node_types:
            name = self._generic_name(node, source, cfg)
            if name:
                owner = parent or self._receiver_owner(node, source, cfg)
                symbols.append(
                    self._generic_function_symbol(
                        node, source, walk, _FunctionSite(name=name, owner=owner, header=node)
                    )
                )
            return []
        if node.type in cfg.binding_node_types:
            return self._visit_generic_binding(node, source, walk, parent)
        if node.type in cfg.constant_node_types:
            self._append_generic_constant(node, source, walk, parent)
            return []
        if node.type in cfg.class_node_types:
            name = self._generic_name(node, source, cfg)
            if not name:
                return []
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
                    is_public=self._generic_is_public(node, source, cfg, name),
                    is_async=False,
                )
            )
            # The class body carries the class as the parent of its methods.
            body = self._class_body(node, cfg)
            return [(child, name) for child in body.children] if body else []
        # Not a declaration: a wrapper (export_statement, a block, an
        # expression) that may still hold one.
        return [(child, parent) for child in node.children]

    def _generic_function_symbol(
        self, node: Node, source: bytes, walk: _GenericWalk, site: _FunctionSite
    ) -> ASTSymbol:
        """Build one function or method symbol from the site it was found at."""
        return ASTSymbol(
            name=site.name,
            kind=SymbolKind.METHOD if site.owner else SymbolKind.FUNCTION,
            module_path=walk.module_path,
            line_number=node.start_point[0] + 1,
            end_line_number=node.end_point[0] + 1,
            signature=self._generic_signature(site.header, source, site.name, walk.cfg),
            docstring="",
            parent_class=site.owner,
            decorators=(),
            bases=(),
            language=walk.language,
            is_public=self._generic_is_public(node, source, walk.cfg, site.name),
            # Three spellings across the pack: a keyword child (ECMAScript), a
            # `function_modifiers` node (Rust), and a modifier keyword beside
            # the visibility one (C#, Kotlin).
            is_async=declares_async(site.header)
            or "async" in self._modifier_keywords(site.header, source, walk.cfg),
        )

    def _visit_generic_binding(
        self,
        node: Node,
        source: bytes,
        walk: _GenericWalk,
        parent: str,
    ) -> list[tuple[Node, str]]:
        """Emit a function symbol for a function-valued binding, or descend.

        A binding whose value is not a function is not a declaration this
        walker knows, so the walk carries on through it rather than stopping:
        `const [a, b] = f()` still holds expressions worth reaching.
        """
        value = node.child_by_field_name("value")
        if value is None or value.type not in walk.cfg.function_value_node_types:
            return [(child, parent) for child in node.children]
        name = self._generic_name(node, source, walk.cfg)
        if name:
            walk.symbols.append(
                self._generic_function_symbol(
                    node, source, walk, _FunctionSite(name=name, owner=parent, header=value)
                )
            )
        return []

    def _append_generic_constant(
        self,
        node: Node,
        source: bytes,
        walk: _GenericWalk,
        parent: str,
    ) -> None:
        """Emit a constant symbol when the declaration carries the keyword.

        The keywords are what separate a constant from mutable state: a Java
        `static final double TAX_RATE` is a constant of the class, and
        `private final double subtotal` is per-instance state that happens not
        to be reassigned.
        """
        cfg = walk.cfg
        required = frozenset(cfg.constant_modifier_keywords)
        if not required <= self._modifier_keywords(node, source, cfg):
            return
        declarator = node.child_by_field_name("declarator")
        name = self._generic_name(declarator if declarator is not None else node, source, cfg)
        if not name:
            return
        walk.symbols.append(
            ASTSymbol(
                name=name,
                kind=SymbolKind.CONSTANT,
                module_path=walk.module_path,
                line_number=node.start_point[0] + 1,
                end_line_number=node.end_point[0] + 1,
                signature=self._header_text(node, source),
                docstring="",
                parent_class=parent,
                decorators=(),
                bases=(),
                language=walk.language,
                is_public=self._generic_is_public(node, source, cfg, name),
                is_async=False,
            )
        )

    def _modifier_keywords(self, node: Node, source: bytes, cfg: LanguageConfig) -> frozenset[str]:
        """Return the modifier keywords a declaration carries, as words.

        Read as text and split, because the pack spells modifiers two ways: a
        container node holding one child per keyword, and one leaf node per
        keyword. The words are the same either way.
        """
        words: set[str] = set()
        for child in node.children:
            if child.type in cfg.modifier_node_types:
                words.update(self._node_text(child, source).split())
        return frozenset(words)

    def _generic_is_public(self, node: Node, source: bytes, cfg: LanguageConfig, name: str) -> bool:
        """Answer `is_public` from the declaration's own keywords where it has them.

        `is_public` is persisted to the tag cache and is what a caller filters
        an overview on, so a private method reported public is a wrong column
        on disk. A language with no visibility keywords falls back to the
        leading-underscore convention, which is the only signal such a
        language gives.
        """
        if cfg.modifier_node_types:
            return not (self._modifier_keywords(node, source, cfg) & NON_PUBLIC_MODIFIERS)
        return not name.startswith("_")

    def _generic_signature(self, node: Node, source: bytes, name: str, cfg: LanguageConfig) -> str:
        """Render `fn name(params) -> result` from whatever fields exist.

        A bare `fn name` tells a reader nothing about arity or types, and the
        map is read as code. Parameter and result text comes straight from the
        source; collapsing it onto one line and capping its length is
        `ASTSymbol`'s job, so every handler gets the same treatment.

        Languages whose grammar exposes no `parameters` field take the header
        form instead: the composed one would print `fn f()` for a function
        with three parameters, which is worse than saying nothing.
        """
        if cfg.signature_from_header:
            return self._header_text(node, source)

        params_node = node.child_by_field_name("parameters")
        params = self._node_text(params_node, source) if params_node else "()"

        result_node = node.child_by_field_name("return_type") or node.child_by_field_name("result")
        # TypeScript's return_type is a `type_annotation`, so its text arrives
        # as ": string"; the arrow already says what the colon would.
        result_text = self._node_text(result_node, source).lstrip(": ") if result_node else ""
        result = f" -> {result_text}" if result_text else ""

        return f"fn {name}{params}{result}"

    def _header_text(self, node: Node, source: bytes) -> str:
        """Return a declaration's own text up to the start of its body.

        Verbatim source, so a reader sees the language's own spelling of the
        signature -- `fun applyTax(amount: Double): Double` rather than a
        composed approximation of it. `ASTSymbol` collapses and caps it.

        The body is found by field where the grammar names one and by node
        type where it does not, which is the whole reason this path exists: a
        grammar with no `body` field has no `parameters` field either, and
        taking the node's whole text would put the function's body in its
        signature.
        """
        body = node.child_by_field_name("body") or next(
            (child for child in node.children if child.type in BODY_BLOCK_NODE_TYPES), None
        )
        end = body.start_byte if body is not None else node.end_byte
        return source[node.start_byte : end].decode("utf-8", errors="replace").strip()

    def _receiver_owner(self, node: Node, source: bytes, cfg: LanguageConfig) -> str:
        """Return the type a method is declared on, from the grammar's receiver.

        The receiver subtree is a parameter list, so the type is found by node
        type rather than by position: ``(c *Config)`` nests the name under a
        `pointer_type` and ``(s *Stack[T])`` under a `generic_type`, and in
        both the first `type_identifier` in the subtree is the owner. Reading
        the whole receiver's text instead would put the receiver variable and
        its pointer star in every method's parent name.
        """
        if cfg.receiver_field is None:
            return ""
        receiver = node.child_by_field_name(cfg.receiver_field)
        if receiver is None:
            return ""
        owner = next(
            (child for child in walk_nodes(receiver) if child.type == "type_identifier"), None
        )
        return self._node_text(owner, source) if owner is not None else ""

    def _class_body(self, node: Node, cfg: LanguageConfig) -> Node | None:
        """Return the node holding a class's members, field-named or not."""
        body = node.child_by_field_name("body")
        if body is not None:
            return body
        if not cfg.class_body_node_types:
            return None
        return next(
            (child for child in node.children if child.type in cfg.class_body_node_types), None
        )

    def _generic_name(self, node: Node, source: bytes, cfg: LanguageConfig) -> str:
        """Extract the identifier name from a node using the configured field."""
        if cfg.name_field:
            name_node = node.child_by_field_name(cfg.name_field)
            if name_node:
                return self._node_text(name_node, source)
        # Fallback: first child whose type names things in this language.
        for child in node.children:
            if child.type in cfg.name_node_types:
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
        self._collect_import_nodes(root, source, imports, cfg)

    def _collect_import_nodes(
        self,
        root: Node,
        source: bytes,
        imports: list[ImportStatement],
        cfg: LanguageConfig,
    ) -> None:
        """Collect import nodes anywhere in the tree (Go nests them in blocks).

        Over `walk_nodes` rather than a recursive descent: the whole tree is
        searched, so a deeply nested expression elsewhere in the file would
        otherwise exhaust the stack before the walk reached the imports.
        """
        for node in walk_nodes(root):
            if node.type not in cfg.import_node_types:
                continue
            path = self._extract_import_path(node, source, cfg)
            if path:
                imports.append(
                    ImportStatement(
                        module=path,
                        names=self._import_names(node, source, cfg),
                        is_relative=path.startswith("."),
                        relative_level=0,
                        line_number=node.start_point[0] + 1,
                        resolved_path="",
                    )
                )

    def _import_names(self, node: Node, source: bytes, cfg: LanguageConfig) -> tuple[str, ...]:
        """Return the names one import statement binds into the importing file.

        The *local* name, so `import { Y as Z }` reports `Z`: what a bare
        reference in this file can spell is the question, and `Y` is not it.

        A namespace import (`import * as ns from "./n"`) binds a module
        object rather than any of its names, so it contributes nothing here --
        the same rule as Go's `import "fmt"` and Python's `import a.b`, and
        the reason `import_name_node_types` is empty for both of those.
        """
        if not cfg.import_name_node_types:
            return ()

        names: list[str] = []
        for child in walk_nodes(node):
            if child.type == "import_specifier":
                # `X` or `Y as Z`: the last identifier is the local binding.
                identifiers = [c for c in child.children if c.type == "identifier"]
                if identifiers:
                    names.append(self._node_text(identifiers[-1], source))
            elif child.type == "import_clause":
                # A default import is a bare identifier directly under the
                # clause; `{ ... }` and `* as ns` are their own node types and
                # are not children of this shape.
                names.extend(
                    self._node_text(c, source) for c in child.children if c.type == "identifier"
                )
        return tuple(dict.fromkeys(names))

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

        # Walk children looking for the expected string node type. Every
        # match under one parent is joined with a dot, because a grammar that
        # spells a dotted path as a run of sibling identifiers -- scala's
        # `import pricing.Money`, csharp's `using App.Money` -- otherwise
        # reports only its first segment, and the import resolves to a package
        # rather than to the file that defines the name.
        target_type = cfg.import_path_node_type or "string"
        direct = [child for child in node.children if child.type == target_type]
        if direct:
            return self._joined_path(direct, source)
        # One level of nesting (e.g. import_statement > string_fragment).
        for child in node.children:
            nested = [grandchild for grandchild in child.children if grandchild.type == target_type]
            if nested:
                return self._joined_path(nested, source)
        return ""

    def _joined_path(self, nodes: list[Node], source: bytes) -> str:
        """Join one run of path segments into a dotted module string."""
        return ".".join(self._strip_quotes(self._node_text(node, source)) for node in nodes)

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
    # Deterministic non-code surfaces (dedicated)
    # ------------------------------------------------------------------

    def _extract_data_symbols(
        self,
        root: Node,
        source: bytes,
        module_path: str,
        symbols: list[ASTSymbol],
        language: str,
    ) -> None:
        """Extract object/table keys from JSON, TOML and YAML."""
        pair_type = {
            "json": "pair",
            "toml": "pair",
            "yaml": "block_mapping_pair",
        }[language]

        if language == "toml":
            for node in walk_nodes(root):
                if node.type != "table":
                    continue
                key = self._data_key(node, source, language)
                if key:
                    parent, name = self._split_qualified_key(key)
                    symbols.append(
                        self._surface_symbol(
                            _SurfaceDeclaration(
                                name=name,
                                kind=SymbolKind.CLASS,
                                node=node,
                                signature=f"table {key}",
                                parent=parent,
                            ),
                            module_path,
                            language,
                        )
                    )

        for node in walk_nodes(root):
            if node.type != pair_type:
                continue
            key = self._data_key(node, source, language)
            if not key:
                continue
            owner = self._data_owner(node, source, language)
            value = node.child_by_field_name("value")
            has_nested_pair = value is not None and self._has_nested_pair(value)
            is_container = value is not None and value.type in {
                "object",
                "array",
                "block_node",
                "flow_node",
            }
            kind = SymbolKind.CLASS if is_container and has_nested_pair else SymbolKind.CONSTANT
            signature = self._node_text(node, source).replace("\n", " ")
            symbols.append(
                self._surface_symbol(
                    _SurfaceDeclaration(
                        name=key,
                        kind=kind,
                        node=node,
                        signature=signature,
                        parent=owner,
                    ),
                    module_path,
                    language,
                )
            )

    def _extract_hcl_symbols(
        self, root: Node, source: bytes, module_path: str, symbols: list[ASTSymbol]
    ) -> None:
        """Extract Terraform/HCL blocks and their attributes."""
        for node in walk_nodes(root):
            if node.type == "block":
                owner, name, signature = self._hcl_block_identity(node, source)
                if name:
                    symbols.append(
                        self._surface_symbol(
                            _SurfaceDeclaration(
                                name=name,
                                kind=SymbolKind.CLASS,
                                node=node,
                                signature=signature,
                                parent=owner,
                            ),
                            module_path,
                            "hcl",
                        )
                    )
                continue
            if node.type != "attribute":
                continue
            name_node = next(
                (child for child in node.named_children if child.type == "identifier"), None
            )
            if name_node is None:
                continue
            name = self._node_text(name_node, source)
            owner = self._hcl_owner(node, source)
            symbols.append(
                self._surface_symbol(
                    _SurfaceDeclaration(
                        name=name,
                        kind=SymbolKind.CONSTANT,
                        node=node,
                        signature=self._node_text(node, source).replace("\n", " "),
                        parent=owner,
                    ),
                    module_path,
                    "hcl",
                )
            )

    def _extract_sql_symbols(
        self, root: Node, source: bytes, module_path: str, symbols: list[ASTSymbol]
    ) -> None:
        """Extract SQL tables, views and columns from CREATE statements."""
        for node in walk_nodes(root):
            if node.type not in {"create_table", "create_view"}:
                continue
            name = self._sql_object_name(node, source)
            if not name:
                continue
            kind = SymbolKind.CLASS if node.type == "create_table" else SymbolKind.TYPE_ALIAS
            signature = self._node_text(node, source).replace("\n", " ")
            symbols.append(
                self._surface_symbol(
                    _SurfaceDeclaration(
                        name=name,
                        kind=kind,
                        node=node,
                        signature=signature,
                    ),
                    module_path,
                    "sql",
                )
            )
            if node.type != "create_table":
                continue
            for child in walk_nodes(node):
                if child.type != "column_definition":
                    continue
                name_node = child.child_by_field_name("name")
                if name_node is None:
                    continue
                column = self._node_text(name_node, source)
                symbols.append(
                    self._surface_symbol(
                        _SurfaceDeclaration(
                            name=column,
                            kind=SymbolKind.CONSTANT,
                            node=child,
                            signature=self._node_text(child, source),
                            parent=name,
                        ),
                        module_path,
                        "sql",
                    )
                )

    @staticmethod
    def _no_imports(
        _root: Node, _source: bytes, _module_path: str, _imports: list[ImportStatement]
    ) -> None:
        """The deterministic non-code surfaces have no module import syntax."""

    @staticmethod
    def _has_nested_pair(node: Node) -> bool:
        """True when ``node`` contains another config mapping pair."""
        return any(
            child.type in {"pair", "block_mapping_pair"}
            for child in walk_nodes(node)
            if child.id != node.id
        )

    def _data_key(self, node: Node, source: bytes, language: str) -> str:
        """Return the key one config pair or TOML table declares."""
        key_node = node.child_by_field_name("key")
        if key_node is None:
            key_node = next(
                (
                    child
                    for child in node.named_children
                    if child.type in {"bare_key", "quoted_key", "dotted_key"}
                ),
                None,
            )
        if key_node is None:
            return ""
        return self._clean_surface_name(self._node_text(key_node, source), language)

    def _data_owner(self, node: Node, source: bytes, language: str) -> str:
        """Return the dotted key path enclosing one config pair."""
        owners: list[str] = []
        parent = node.parent
        while parent is not None:
            if parent.type in {"pair", "block_mapping_pair", "table"}:
                key = self._data_key(parent, source, language)
                if key:
                    owners.append(key)
            parent = parent.parent
        return ".".join(reversed(owners))

    def _hcl_block_identity(self, node: Node, source: bytes) -> tuple[str, str, str]:
        """Return ``(owner, name, header)`` for one HCL block."""
        parts = [
            self._clean_surface_name(self._node_text(child, source), "hcl")
            for child in node.named_children
            if child.type in {"identifier", "string_lit"}
        ]
        if not parts:
            return "", "", ""
        name = parts[-1]
        owner = ".".join(parts[:-1])
        header = self._node_text(node, source).partition("{")[0].strip()
        return owner, name, header

    def _hcl_owner(self, node: Node, source: bytes) -> str:
        """Return the qualified identity of the nearest enclosing HCL block."""
        parent = node.parent
        while parent is not None:
            if parent.type == "block":
                owner, name, _ = self._hcl_block_identity(parent, source)
                return ".".join(part for part in (owner, name) if part)
            parent = parent.parent
        return ""

    def _sql_object_name(self, node: Node, source: bytes) -> str:
        """Return the directly declared table or view name."""
        reference = next(
            (child for child in node.named_children if child.type == "object_reference"), None
        )
        if reference is None:
            return ""
        name = reference.child_by_field_name("name")
        return self._node_text(name or reference, source)

    @staticmethod
    def _clean_surface_name(text: str, language: str) -> str:
        """Normalize a parsed config key or HCL label without guessing."""
        cleaned = text.strip()
        if (
            language in {"json", "hcl", "toml"}
            and len(cleaned) >= _QUOTED_TEXT_MIN_CHARS
            and cleaned[0] == cleaned[-1]
            and cleaned[0] in {'"', "'"}
        ):
            return cleaned[1:-1]
        return cleaned

    @staticmethod
    def _split_qualified_key(key: str) -> tuple[str, str]:
        """Split a dotted TOML table key into owner and local name."""
        owner, separator, name = key.rpartition(".")
        return (owner, name) if separator else ("", key)

    @staticmethod
    def _surface_symbol(
        declaration: _SurfaceDeclaration,
        module_path: str,
        language: str,
    ) -> ASTSymbol:
        """Build one symbol from a deterministic non-code syntax node."""
        return ASTSymbol(
            name=declaration.name,
            kind=declaration.kind,
            module_path=module_path,
            line_number=declaration.node.start_point[0] + 1,
            end_line_number=declaration.node.end_point[0] + 1,
            signature=declaration.signature,
            docstring="",
            parent_class=declaration.parent,
            decorators=(),
            bases=(),
            language=language,
            is_public=True,
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
        """Extract function and tagged-type symbols from a C/C++ AST.

        Over `declarations_under`, so an include guard and a C++ namespace are
        descended through rather than treated as the end of the file: an
        `#ifndef` wraps a whole header in one `preproc_ifdef`, which is the
        normal shape of a C header and used to hide every symbol in it.
        """
        for node in declarations_under(root, C_DECLARATION_TYPES):
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
            else:
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
        """Extract #include directives as import statements.

        Over the whole tree, the same as the generic import walk: an include
        inside a guard is one level deeper than the root, and a header that
        contributed no edges to the import graph was the result.
        """
        _ = module_path
        for node in walk_nodes(root):
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
                                # An `#include` is a textual paste: every name
                                # the header declares is in this translation
                                # unit's namespace afterwards, unqualified.
                                binds_all=True,
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

    def _walk_require(self, root: Node, source: bytes, imports: list[ImportStatement]) -> None:
        """Walk the tree collecting require() calls.

        Over `walk_nodes` rather than a recursive descent, for the reason that
        function gives: a Lua module ending in a long method chain must not
        turn its own import list into a `RecursionError`.
        """
        for node in walk_nodes(root):
            if node.type not in ("call", "call_expression", "function_call"):
                continue
            fn_node = node.child_by_field_name("function") or (
                node.children[0] if node.children else None
            )
            if fn_node is None or self._node_text(fn_node, source) != "require":
                continue
            args = node.child_by_field_name("arguments")
            if args is None:
                continue
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

    def _walk_bash_source(self, root: Node, source: bytes, imports: list[ImportStatement]) -> None:
        """Walk the tree collecting source/dot commands.

        Over `walk_nodes` rather than a recursive descent: nested command
        substitutions nest the tree as deeply as a script cares to, and one
        script must not take the repository index down with it.
        """
        for node in walk_nodes(root):
            if node.type != "command":
                continue
            children = list(node.children)
            if not children:
                continue
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
        """Extract symbols from a Python AST.

        Over `declarations_under`, so a definition guarded by `if
        TYPE_CHECKING:` or by a version check is a symbol of the module that
        makes it, not one hidden by the block it sits in.
        """
        for child in declarations_under(root, PYTHON_DECLARATION_TYPES):
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

        is_async = declares_async(node)
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
        """Extract symbols from a Rust AST.

        Over `declarations_under`, so a `mod` body contributes its items --
        including `#[cfg(test)] mod tests`, which is how a Rust crate writes
        its tests and which used to be invisible to the symbol map entirely.
        """
        for child in declarations_under(root, RUST_ITEM_TYPES):
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

        is_async = declares_async(node)
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
        """Extract use declarations from a Rust AST.

        Over the whole tree, so a `use super::*;` inside a `mod` block reaches
        the import graph.
        """
        _ = module_path
        for child in walk_nodes(root):
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
        """Extract import statements from a Python AST.

        Over the whole tree, the same as the generic import walk: an import
        under `if TYPE_CHECKING:` is exactly the one a repository puts there
        to avoid a cycle, and it never reached the import graph.
        """
        _ = module_path
        for child in walk_nodes(root):
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
                alias_node = child.child_by_field_name("alias")
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
                            # `import a.b as ab` binds `ab` to the submodule,
                            # not `a` to the package. Dropping the alias lost
                            # both halves of that.
                            alias=self._node_text(alias_node, source) if alias_node else "",
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
        names, local_names = self._collect_import_names(node, module_node, source)

        imports.append(
            ImportStatement(
                module=module_name,
                names=tuple(names),
                local_names=tuple(local_names),
                is_relative=relative_level > 0,
                relative_level=relative_level,
                line_number=node.start_point[0] + 1,
                resolved_path="",
                # `from x import *` is the one Python form that genuinely
                # binds every name the target defines. It is recorded as the
                # name `*`, which no reference ever spells, so without this
                # the one import that does bind wholesale supplied no
                # evidence at all while `import x` -- which binds none --
                # supplied it for everything.
                binds_all="*" in names,
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
    ) -> tuple[list[str], list[str]]:
        """Collect the members a 'from' import names and the names they bind.

        Two lists rather than one, positionally paired: `from pkg import mod
        as m` needs `mod` to resolve the member and `m` to record what this
        file can spell. Recording only the first left `m` bound to nothing.
        """
        names: list[str] = []
        local_names: list[str] = []
        for child in node.children:
            if child.type == "dotted_name" and child != module_node:
                text = self._node_text(child, source)
                names.append(text)
                local_names.append(text)
            elif child.type == "aliased_import":
                name_node = child.child_by_field_name("name")
                alias_node = child.child_by_field_name("alias")
                if name_node:
                    names.append(self._node_text(name_node, source))
                    local_names.append(
                        self._node_text(alias_node, source)
                        if alias_node
                        else self._node_text(name_node, source)
                    )
            elif child.type == "wildcard_import":
                names.append("*")
                local_names.append("*")
        return names, local_names

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
