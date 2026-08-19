"""Stable ids: the format, and the round trip the two-call recipe depends on."""

import pytest

from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import (
    LANGUAGE_PREFIXES,
    ASTSymbol,
    SymbolKind,
    disambiguate,
    id_qualname,
    parse_stable_id,
    qualname,
    split_ordinal,
    stable_id,
    symbol_stable_id,
)

SOURCE = "class Invoice:\n    def total(self):\n        return 1\n\n\ndef helper():\n    return 2\n"


class TestFormat:
    def test_the_documented_example_renders_exactly(self):
        assert (
            stable_id("python", "src/app/svc.py", "Invoice.total")
            == "py:src/app/svc.py::Invoice.total"
        )

    def test_a_method_qualname_carries_its_class(self):
        symbols = TreeSitterExtractor().extract_from_source(SOURCE, "python", "billing.py")
        total = next(symbol for symbol in symbols if symbol.name == "total")
        assert qualname(total) == "Invoice.total"
        assert symbol_stable_id(total) == "py:billing.py::Invoice.total"

    def test_a_module_function_has_no_class_component(self):
        symbols = TreeSitterExtractor().extract_from_source(SOURCE, "python", "billing.py")
        helper = next(symbol for symbol in symbols if symbol.name == "helper")
        assert symbol_stable_id(helper) == "py:billing.py::helper"

    def test_an_unlisted_language_falls_back_to_its_own_name(self):
        assert stable_id("zig", "a.zig", "main") == "zig:a.zig::main"

    def test_every_supported_language_has_a_prefix(self):
        supported = set(TreeSitterExtractor.SUPPORTED_EXTENSIONS.values())
        assert supported <= set(LANGUAGE_PREFIXES)


class TestRoundTrip:
    @pytest.mark.parametrize(
        ("language", "path", "name"),
        [
            ("python", "src/app/svc.py", "Invoice.total"),
            ("typescript", "src/pricing.ts", "PriceBook.quote"),
            ("go", "internal/store/db.go", "Open"),
            ("python", "a-b/c.d.py", "x"),
        ],
    )
    def test_build_then_parse_returns_the_parts(self, language, path, name):
        parsed = parse_stable_id(stable_id(language, path, name))
        assert (parsed.path, parsed.qualname) == (path, name)

    def test_str_renders_back_to_the_wire_form(self):
        text = "py:src/app/svc.py::Invoice.total"
        assert str(parse_stable_id(text)) == text

    @pytest.mark.parametrize(
        "malformed",
        ["", "py", "py:src/app.py", "src/app.py::X", "::X", "py:::X", "py:src/app.py::"],
    )
    def test_a_malformed_id_raises_rather_than_guessing(self, malformed):
        with pytest.raises(ValueError, match="not a stable id"):
            parse_stable_id(malformed)


GO_RECEIVERS = """package config

type ServerInfo struct{ Host string }

type AWSConf struct{ Region string }

func (s ServerInfo) Validate() error {
\treturn nil
}

func (a *AWSConf) Validate() error {
\treturn nil
}

func Validate() error {
\treturn nil
}
"""

# Two functions with one name in one file and no grammar-supplied owner: the
# shape C++ overloads, reopened Ruby classes and sibling namespaces all reduce
# to. Python because the suite warms its grammar, not because it is special.
COLLIDING = "def handle():\n    return 1\n\n\ndef handle():\n    return 2\n"


class TestGoReceivers:
    """A Go method's owner is its receiver type, so ids stay unique."""

    def test_same_named_methods_on_different_receivers_get_distinct_ids(self, extractor):
        symbols = extractor.extract_from_source(GO_RECEIVERS, "go", "config/config.go")
        ids = [symbol_stable_id(symbol) for symbol in symbols if symbol.name == "Validate"]
        assert ids == [
            "go:config/config.go::ServerInfo.Validate",
            "go:config/config.go::AWSConf.Validate",
            "go:config/config.go::Validate",
        ]

    def test_a_pointer_receiver_names_the_type_not_the_pointer(self, extractor):
        symbols = extractor.extract_from_source(GO_RECEIVERS, "go", "config/config.go")
        pointer = next(symbol for symbol in symbols if symbol.parent_class == "AWSConf")
        assert pointer.kind is SymbolKind.METHOD

    def test_a_plain_function_keeps_no_owner(self, extractor):
        symbols = extractor.extract_from_source(GO_RECEIVERS, "go", "config/config.go")
        plain = [symbol for symbol in symbols if not symbol.parent_class]
        assert [symbol.name for symbol in plain] == ["Validate"]
        assert plain[0].kind is SymbolKind.FUNCTION


class TestDuplicateBackstop:
    """Where no grammar context disambiguates, a source-order ordinal does."""

    def test_a_repeated_qualified_name_is_numbered_from_the_second(self, extractor):
        symbols = extractor.extract_from_source(COLLIDING, "python", "handlers.py")
        assert [symbol_stable_id(symbol) for symbol in symbols] == [
            "py:handlers.py::handle",
            "py:handlers.py::handle#2",
        ]

    def test_the_displayed_name_never_carries_the_ordinal(self, extractor):
        symbols = extractor.extract_from_source(COLLIDING, "python", "handlers.py")
        assert [qualname(symbol) for symbol in symbols] == ["handle", "handle"]

    def test_ordinals_follow_the_source_and_not_the_traversal(self):
        def at(line, name, parent=""):
            return ASTSymbol(
                name=name,
                kind=SymbolKind.METHOD,
                module_path="a.py",
                line_number=line,
                end_line_number=line,
                signature=f"fn {name}",
                docstring="",
                parent_class=parent,
                decorators=(),
                bases=(),
                language="python",
                is_public=True,
                is_async=False,
            )

        # Emitted out of source order, as a class-body-then-recurse walk does.
        numbered = disambiguate([at(90, "run"), at(10, "run"), at(50, "run")])
        assert [(symbol.line_number, id_qualname(symbol)) for symbol in numbered] == [
            (90, "run#3"),
            (10, "run"),
            (50, "run#2"),
        ]

    def test_an_owner_of_its_own_is_enough_to_avoid_an_ordinal(self, extractor):
        source = (
            "class A:\n    def run(self):\n        pass\n\n\n"
            "class B:\n    def run(self):\n        pass\n"
        )
        symbols = extractor.extract_from_source(source, "python", "a.py")
        assert [symbol_stable_id(symbol) for symbol in symbols if symbol.name == "run"] == [
            "py:a.py::A.run",
            "py:a.py::B.run",
        ]

    @pytest.mark.parametrize(
        ("text", "expected"),
        [
            ("Invoice.total", ("Invoice.total", 0)),
            ("Invoice.total#2", ("Invoice.total", 1)),
            ("Invoice.total#10", ("Invoice.total", 9)),
            ("total#0", ("total", 0)),
            ("C#Sharp", ("C#Sharp", 0)),
        ],
    )
    def test_split_ordinal_inverts_the_spelling(self, text, expected):
        assert split_ordinal(text) == expected
