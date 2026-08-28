"""Tests for tree-sitter AST symbol extraction.

Ported verbatim (bar module paths) from mcp-local's
``tests/unit/test_tree_sitter_extractor.py``. They are the drift tripwire:
mcp-local pins per-grammar PyPI packages, this package takes whatever
revision tree-sitter-language-pack ships, and these assertions are what says
the two still agree.

Grammar drift found and fixed on the port (tree-sitter-language-pack 1.14.3,
python grammar ABI 14): the revision the pack ships no longer wraps
module-level statements in an ``expression_statement`` node. Module constants
arrive as a bare ``assignment`` and docstrings as a bare ``string``, so
``_extract_python_symbols`` and ``_get_docstring`` in
``agentless_mcp.core.extractor`` now accept both shapes. Without that fix
``test_uppercase_constant`` and every docstring assertion below fail.
"""

import pytest

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import TreeSitterExtractor, _scope_tree, walk_nodes
from agentless_mcp.core.symbols import SymbolKind, rationale_stable_id

SAMPLE_PYTHON = '''\
"""Module docstring."""

MAX_SIZE = 100
_INTERNAL = "private"

def greet(name: str) -> str:
    """Say hello."""
    return f"Hello, {name}"

async def fetch_data(url: str) -> dict:
    """Fetch data from URL."""
    pass

class Animal:
    """Base animal class."""

    def speak(self) -> str:
        """Make a sound."""
        return ""

    async def _internal(self) -> None:
        pass

class Dog(Animal):
    """A dog."""

    def speak(self) -> str:
        """Bark."""
        return "Woof"
'''


def make_extractor():
    return TreeSitterExtractor()


class TestRationaleExtraction:
    def test_comment_markers_and_citations_attach_to_the_innermost_symbol(self):
        source = """\
class Planner:
    def choose(self, value):
        decoy = "TODO: this string is not a comment"
        # WHY: preserve stable ordering; see ADR-004
        # RFC 2119 requires this branch.
        return value
"""

        symbols = make_extractor().extract_from_source(source, "python", "planner.py")
        method = next(symbol for symbol in symbols if symbol.name == "choose")
        owner = next(symbol for symbol in symbols if symbol.name == "Planner")

        assert [(node.kind, node.text, node.citations) for node in method.rationales] == [
            ("why", "preserve stable ordering; see ADR-004", ("ADR-004",)),
            ("citation", "RFC 2119 requires this branch.", ("RFC 2119",)),
        ]
        assert owner.rationales == ()
        assert rationale_stable_id(method, method.rationales[0]) == (
            "py:planner.py::Planner.choose::rationale@4"
        )

    def test_comment_text_is_bounded_before_it_leaves_extraction(self):
        source = "def choose():\n    # NOTE: " + "x" * 500 + "\n    return 1\n"

        (symbol,) = make_extractor().extract_from_source(source, "python", "planner.py")

        assert len(symbol.rationales[0].text) == 240
        assert symbol.rationales[0].text.endswith("...")


class TestFunctionExtraction:
    def test_module_level_function(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        funcs = [s for s in symbols if s.kind == SymbolKind.FUNCTION]
        assert len(funcs) == 2
        greet = funcs[0]
        assert greet.name == "greet"
        assert "name: str" in greet.signature
        assert "-> str" in greet.signature
        assert greet.docstring == "Say hello."
        assert greet.is_public is True
        assert greet.is_async is False
        assert greet.parent_class == ""

    def test_async_function(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        funcs = [s for s in symbols if s.kind == SymbolKind.FUNCTION]
        fetch = next(f for f in funcs if f.name == "fetch_data")
        assert fetch.is_async is True
        assert "async def" in fetch.signature


class TestClassExtraction:
    def test_class_and_methods(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        classes = [s for s in symbols if s.kind == SymbolKind.CLASS]
        assert len(classes) == 2

        animal = classes[0]
        assert animal.name == "Animal"
        assert animal.docstring == "Base animal class."
        assert animal.is_public is True

        dog = classes[1]
        assert dog.name == "Dog"
        assert dog.bases == ("Animal",)

    def test_methods(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        methods = [s for s in symbols if s.kind == SymbolKind.METHOD]
        assert len(methods) >= 3
        speak_methods = [m for m in methods if m.name == "speak"]
        assert len(speak_methods) == 2
        assert speak_methods[0].parent_class == "Animal"
        assert speak_methods[1].parent_class == "Dog"

    def test_private_method(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        internal = next(s for s in symbols if s.name == "_internal")
        assert internal.is_public is False
        assert internal.kind == SymbolKind.METHOD


class TestConstantExtraction:
    def test_uppercase_constant(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        constants = [s for s in symbols if s.kind == SymbolKind.CONSTANT]
        assert len(constants) == 1
        assert constants[0].name == "MAX_SIZE"
        assert "100" in constants[0].signature

    def test_private_constant_excluded(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        constants = [s for s in symbols if s.kind == SymbolKind.CONSTANT]
        names = {c.name for c in constants}
        assert "_INTERNAL" not in names


class TestDecoratorDetection:
    def test_dataclass(self):
        source = '''\
from dataclasses import dataclass

@dataclass
class Config:
    """Configuration."""
    host: str
    port: int
'''
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "python", "test.py")
        config = next(s for s in symbols if s.name == "Config")
        assert config.kind == SymbolKind.DATACLASS
        assert "dataclass" in config.decorators

    def test_protocol(self):
        source = '''\
from typing import Protocol

class Readable(Protocol):
    """Something readable."""
    def read(self) -> str: ...
'''
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "python", "test.py")
        readable = next(s for s in symbols if s.name == "Readable")
        assert readable.kind == SymbolKind.PROTOCOL

    def test_enum(self):
        source = """\
from enum import StrEnum

class Color(StrEnum):
    RED = "red"
    GREEN = "green"
"""
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "python", "test.py")
        color = next(s for s in symbols if s.name == "Color")
        assert color.kind == SymbolKind.ENUM


class TestEdgeCases:
    def test_empty_file(self):
        ext = make_extractor()
        symbols = ext.extract_from_source("", "python", "test.py")
        assert symbols == []

    def test_syntax_error_tolerance(self):
        source = "def broken(:\n    pass\n"
        ext = make_extractor()
        # tree-sitter is error-tolerant, should not raise
        symbols = ext.extract_from_source(source, "python", "test.py")
        assert isinstance(symbols, list)

    def test_unsupported_language(self):
        ext = make_extractor()
        symbols = ext.extract_from_source("IDENTIFICATION DIVISION.", "cobol", "test.cbl")
        assert symbols == []

    def test_unsupported_language_yields_no_imports(self):
        ext = make_extractor()
        imports = ext.extract_imports_from_source("IDENTIFICATION DIVISION.", "cobol", "test.cbl")
        assert imports == []

    def test_a_grammar_that_fails_to_load_is_not_an_unsupported_language(self, monkeypatch):
        """An ABI-incompatible grammar must surface, not read as an empty file.

        `tree_sitter.Parser` raises `ValueError` for a grammar built against an
        incompatible ABI, which is the same class `_grammar_of` used to raise
        for a language that is simply not in the registry. Laundering the first
        into the second reports a repository of unparsed files as a repository
        of empty ones.
        """

        def incompatible(name):
            message = f"Incompatible Language version for {name}"
            raise ValueError(message)

        monkeypatch.setattr(grammars, "get_parser", incompatible)
        ext = make_extractor()

        with pytest.raises(ValueError, match="Incompatible Language version"):
            ext.extract_from_source("def f(): pass\n", "python", "test.py")
        with pytest.raises(ValueError, match="Incompatible Language version"):
            ext.extract_imports_from_source("import os\n", "python", "test.py")

    def test_module_path_preserved(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "src/foo/bar.py")
        for s in symbols:
            assert s.module_path == "src/foo/bar.py"

    def test_language_set(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(SAMPLE_PYTHON, "python", "test.py")
        for s in symbols:
            assert s.language == "python"


class TestOtherLanguages:
    """Not in the mcp-local suite: proof the pack's other grammars still fit
    the LanguageConfig table this package inherited."""

    def test_javascript_class_and_function(self):
        source = "class Widget {\n  render() { return 1; }\n}\n\nfunction build(x) { return x; }\n"
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "javascript", "widget.js")
        names = {(s.name, s.kind) for s in symbols}
        assert ("Widget", SymbolKind.CLASS) in names
        assert ("build", SymbolKind.FUNCTION) in names

    def test_go_functions_and_imports(self):
        source = 'package main\n\nimport "fmt"\n\nfunc Add(a int, b int) int { return a + b }\n'
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "go", "add.go")
        assert [s.name for s in symbols if s.kind == SymbolKind.FUNCTION] == ["Add"]
        imports = ext.extract_imports_from_source(source, "go", "add.go")
        assert [i.module for i in imports] == ["fmt"]

    def test_typescript_interface(self):
        source = "interface Money {\n  amount: number;\n}\n"
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "typescript", "money.ts")
        assert [s.name for s in symbols if s.kind == SymbolKind.CLASS] == ["Money"]

    def test_exported_declarations_are_seen(self):
        """Fixed in Phase 1b (was a ported mcp-local limitation).

        `export class X` and `export function f` are wrapped in an
        `export_statement`, and the old traversal only looked at the root's
        direct children, so an entire TypeScript module of exports extracted
        as nothing. The walk now descends through non-declaration nodes.
        """
        source = (
            "export class Widget {}\n\nexport function build(x: number): number {\n  return x;\n}\n"
        )
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "typescript", "widget.ts")
        assert {(s.name, s.kind) for s in symbols} == {
            ("Widget", SymbolKind.CLASS),
            ("build", SymbolKind.FUNCTION),
        }

    def test_class_methods_are_seen_with_their_parent(self):
        source = (
            "export class Widget {\n  render(depth: number): string {\n    return '';\n  }\n}\n"
        )
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "typescript", "widget.ts")
        render = next(s for s in symbols if s.name == "render")
        assert render.kind == SymbolKind.METHOD
        assert render.parent_class == "Widget"
        assert render.signature == "fn render(depth: number) -> string"

    def test_a_closure_inside_a_body_is_not_a_top_level_symbol(self):
        source = "function outer() {\n  function inner() { return 1; }\n  return inner;\n}\n"
        ext = make_extractor()
        symbols = ext.extract_from_source(source, "javascript", "outer.js")
        assert [s.name for s in symbols] == ["outer"]


# A minified bundle or a generated client chains this deeply as a matter of
# course; measured, the recursive walkers failed at 248 chained calls under the
# default recursion limit, so 600 is a shape a real repository ships rather
# than an adversarial one.
_DEEP_CHAIN = 600

DEEP_JS = (
    "import defaults from './defaults.js';\n"
    "const value = client" + "".join(f".step{index}()" for index in range(_DEEP_CHAIN)) + ";\n"
    "function build(x) { return x; }\n"
)

# The same shape in Python, which reaches a walker the JavaScript sample
# cannot. `_scope_tree` runs only for Python, behind `_python_roles`, so every
# deep fixture in this suite -- DEEP_JS here, DEEP_LUA and DEEP_BASH in
# test_tier2_languages.py -- left it undriven at depth.
DEEP_PY = (
    "import defaults\n"
    "value = client" + "".join(f".step{index}()" for index in range(_DEEP_CHAIN)) + "\n"
    "def build(x):\n"
    "    return x\n"
)


class TestStackSafety:
    """Every walk in this module is iterative, and this is what says so.

    `walk_nodes` documents the rule -- a deeply nested expression in a
    generated file must not turn a repository map into a `RecursionError` --
    and each test below drives one walker that used to recurse per child. A
    `RecursionError` escaping any of them aborts the whole repository index,
    because the scan reaches these through a single per-file call.
    """

    def test_a_deep_call_chain_still_extracts_symbols(self):
        ext = make_extractor()
        symbols = ext.extract_from_source(DEEP_JS, "javascript", "bundle.js")
        assert [s.name for s in symbols] == ["build"]

    def test_a_deep_call_chain_still_extracts_imports(self):
        ext = make_extractor()
        imports = ext.extract_imports_from_source(DEEP_JS, "javascript", "bundle.js")
        assert [i.module for i in imports] == ["./defaults.js"]

    def test_a_deep_python_chain_still_collects_references(self):
        """The Python-only walkers, which no other deep fixture reaches.

        A reference pass over Python runs `_python_roles`, and that builds a
        `_ScopeTree` by walking the whole parse tree. Every other stack-safety
        fixture in this suite is a language with no scope analysis, so the
        chain below is the only thing that drives `_scope_tree` deeper than a
        handful of levels. Both ends of the chain are asserted because a walk
        that stopped early would still return references, just fewer of them,
        and a partial reference table reads as "this symbol is unused".
        """
        ext = make_extractor()
        refs = ext.extract_refs_from_source(DEEP_PY, "python", "deep.py")
        names = {ref.name for ref in refs}
        assert {"client", "build", "step0", f"step{_DEEP_CHAIN - 1}"} <= names


NESTED_SCOPES_PY = """\
value = 1


class Outer:
    def method(self, arg):
        def inner(x):
            def deepest(y):
                return x + y + value

            return deepest(arg)

        return inner(arg)


def after(z):
    return z
"""


def _function_ids(root):
    """Every ``function_definition`` id at or under ``root``, by ``.children``.

    A `.children` descent rather than a `walk_nodes` call. This builds the
    boundary set the subject is measured against, so reaching for the subject
    to build it would make the comparison circular.
    """
    found = []
    stack = [root]
    while stack:
        node = stack.pop()
        if node.type == "function_definition":
            found.append(node.id)
        stack.extend(reversed(node.children))
    return frozenset(found)


def _children_scope_maps(root, boundary_ids):
    """The pre-cursor ``_scope_tree``, over ``node.children``, as the oracle.

    A stack of `(node, inherited boundary)` pairs, which is exactly what the
    cursor version replaced with a boundary stack pushed on descent. Pushing
    `reversed(node.children)` reproduces the pre-order that
    `goto_first_child` / `goto_next_sibling` produces.

    Duplicated on purpose. An oracle that shared code with the subject, or
    that called `walk_nodes` to enumerate, would agree with it by construction
    and could not fail.
    """
    boundary_of = {}
    outer_of = {}
    node_of = {}
    stack = [(root, None)]
    while stack:
        node, inherited = stack.pop()
        innermost = inherited
        if node.id in boundary_ids:
            outer_of[node.id] = inherited
            node_of[node.id] = node
            innermost = node.id
        boundary_of[node.id] = innermost
        for child in reversed(node.children):
            stack.append((child, innermost))
    return boundary_of, outer_of, node_of


class TestTheCursorWalksAgreeWithTheChildrenWalk:
    """The cursor rewrite kept the old traversal's answers, and this says so.

    `walk_nodes` and `_scope_tree` both moved from a `node.children` stack to
    a `TreeCursor`, for the speed their docstrings measure. A cursor is driven
    by position rather than by a materialized child list, so it can differ
    from the old walk in two ways: it could visit the same nodes in a
    different order, or it could climb out of the subnode it was created from
    and index the whole file. Both faults are silent, and both would move
    every view this package renders.
    """

    def test_a_scope_tree_rooted_at_a_nested_node_matches_a_children_walk(self):
        """Rooted three levels in, because that is where a cursor can escape.

        `_scope_tree` builds its cursor with `root.walk()`, and `goto_parent`
        returning false at the origin is what stops the walk from climbing
        past it. Most callers pass a subnode, so a cursor that escaped would
        map the whole file under a method's boundaries and no assertion on a
        whole-file walk would notice.
        """
        tree = grammars.get_parser("python").parse(NESTED_SCOPES_PY.encode("utf-8"))
        module = tree.root_node
        outer = next(child for child in module.children if child.type == "class_definition")
        body = outer.child_by_field_name("body")
        method = next(child for child in body.children if child.type == "function_definition")

        boundaries = _function_ids(method)
        built = _scope_tree(method, boundaries)
        expected_boundary_of, expected_outer_of, expected_node_of = _children_scope_maps(
            method, boundaries
        )

        # method, inner, deepest: a chain deep enough that `outer_of` has to
        # carry a boundary through two levels rather than straight to None.
        assert len(boundaries) == 3
        assert built.boundary_of == expected_boundary_of
        assert built.outer_of == expected_outer_of
        # Projected rather than compared as nodes, so the assertion rests on
        # the tree's own facts and not on `Node.__eq__`.
        assert {nid: (n.type, n.start_byte) for nid, n in built.node_of.items()} == {
            nid: (n.type, n.start_byte) for nid, n in expected_node_of.items()
        }

        top_level = [child for child in module.children if child.type == "function_definition"]
        assert len(top_level) == 1, "the fixture must keep exactly one top-level function"
        assert module.id not in built.boundary_of
        assert top_level[0].id not in built.boundary_of

    def test_a_childless_root_walks_to_exactly_itself(self):
        """The termination path: no first child, no sibling, no parent to climb.

        Asserted at two positions, because they fail differently. An empty
        module is a tree root, where returning more than itself means the
        walk invented nodes. A leaf is a subnode, where returning more than
        itself means `goto_parent` succeeded at the origin and the walk
        escaped into the rest of the file.
        """
        parser = grammars.get_parser("python")

        empty_tree = parser.parse(b"")
        empty = empty_tree.root_node
        assert empty.children == []
        assert [node.id for node in walk_nodes(empty)] == [empty.id]

        leaf_tree = parser.parse(b"x = 1")
        leaf = leaf_tree.root_node
        # Descended rather than indexed by node type. This file's header
        # records a grammar drift that added and then dropped a wrapper at
        # exactly this position, and a hardcoded shape would fail on the next.
        while leaf.children:
            leaf = leaf.children[0]
        assert [node.id for node in walk_nodes(leaf)] == [leaf.id]


class TestTheDecisionsBranchCoverageLeftUnpinned:
    """Real constructs the suite never drove, measured rather than guessed.

    Each one below was a partial branch in this module: a shape the grammar
    produces, reached by no test. None of them turned out to be broken, so
    what follows is a pin on behaviour that was correct by luck and had
    nothing holding it there.
    """

    def test_a_global_declaration_stops_a_name_resolving_to_the_import(self):
        """The highest-value gap of the set, because it decides an evidence tier.

        ``global json`` rebinds the name inside the function, so a later use is
        the module-level variable and not the imported module. Without this
        branch the use reads as ``module_qualifier`` and the resolver hands an
        agent ``resolved-via-import`` for a symbol the import never named --
        the strongest evidence tier this tool publishes, on a relationship that
        does not exist.
        """
        ext = make_extractor()
        visible = "import json\n\ndef f():\n    return json.loads\n"
        shadowed = "import json\n\ndef f():\n    global json\n    json = 1\n    return json\n"

        roles = [r.role.value for r in ext.extract_refs_from_source(visible, "python", "g.py")]
        assert "module_qualifier" in roles

        after = ext.extract_refs_from_source(shadowed, "python", "g.py")
        assert [r.role.value for r in after if r.line == 6] == ["reference"]
        assert "module_qualifier" not in [r.role.value for r in after]

    def test_parameter_separators_are_not_mistaken_for_parameters(self):
        """``/`` and ``*`` are named nodes in the parameter list, not identifiers."""
        ext = make_extractor()
        source = "def f(a, /, b, *, c):\n    return a + b + c\n"

        bound = [r.name for r in ext.extract_refs_from_source(source, "python", "p.py")]

        assert bound == ["f", "a", "b", "c", "a", "b", "c"]

    def test_a_nonlocal_declaration_binds_rather_than_references(self):
        """``nonlocal total`` says where ``total`` lives; it does not read it."""
        ext = make_extractor()
        source = (
            "def outer():\n"
            "    total = 0\n"
            "    def inner():\n"
            "        nonlocal total\n"
            "        total += 1\n"
            "    return inner\n"
        )

        roles = {
            (r.name, r.line): r.role.value
            for r in ext.extract_refs_from_source(source, "python", "n.py")
        }

        assert roles[("total", 4)] == "binding"

    @pytest.mark.parametrize(
        ("source", "language", "path"),
        [
            ("int f(void) {\n  /* NOTE: keep the id stable */\n  return 1;\n}\n", "c", "a.c"),
            (
                "function f() {\n  // NOTE: keep the id stable\n  return 1;\n}\n",
                "javascript",
                "a.js",
            ),
            ("def f():\n    # NOTE: keep the id stable\n    return 1\n", "python", "a.py"),
        ],
        ids=["block-comment", "line-comment", "hash-comment"],
    )
    def test_comment_delimiters_are_stripped_from_both_ends(self, source, language, path):
        """A trailing ``*/`` in the rationale text is the delimiter leaking through."""
        ext = make_extractor()

        symbols = ext.extract_from_source(source, language, path)

        rationales = [r for symbol in symbols for r in symbol.rationales]
        assert [r.text for r in rationales] == ["keep the id stable"]

    def test_a_rationale_inside_no_symbol_is_dropped(self):
        """Characterization, not endorsement: a module-level NOTE reaches nobody.

        Rationales attach to the symbol that contains them, and a comment above
        every symbol is contained in none. Pinned so that giving module-level
        comments a home later is a visible change rather than an accident.
        """
        ext = make_extractor()
        source = "# NOTE: a module-level rationale\ndef f():\n    return 1\n"

        symbols = ext.extract_from_source(source, "python", "b.py")

        assert [symbol.name for symbol in symbols] == ["f"]
        assert not [r for symbol in symbols for r in symbol.rationales]
