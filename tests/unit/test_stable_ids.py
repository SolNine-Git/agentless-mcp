"""Stable ids: the format, and the round trip the two-call recipe depends on."""

import pytest

from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import (
    LANGUAGE_PREFIXES,
    parse_stable_id,
    qualname,
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
