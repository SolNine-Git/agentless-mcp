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
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import SymbolKind

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
