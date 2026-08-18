"""The SymbolKind 3.10 shim, and the one-line signature invariant."""

import json

import pytest

from agentless_mcp.core.symbols import (
    SIGNATURE_MAX_CHARS,
    ASTSymbol,
    SymbolKind,
    normalise_signature,
)


def symbol(signature):
    """Build a symbol carrying ``signature`` and nothing else of interest."""
    return ASTSymbol(
        name="f",
        kind=SymbolKind.FUNCTION,
        module_path="m.py",
        line_number=1,
        end_line_number=2,
        signature=signature,
        docstring="",
        parent_class="",
        decorators=(),
        bases=(),
        language="python",
        is_public=True,
        is_async=False,
    )


class TestSymbolKind:
    def test_member_equals_its_value(self):
        assert SymbolKind.CLASS == "class"
        assert SymbolKind.METHOD == "method"

    def test_str_returns_the_value_like_strenum(self):
        assert str(SymbolKind.TYPE_ALIAS) == "type_alias"

    def test_format_returns_the_value_like_strenum(self):
        assert f"{SymbolKind.PROTOCOL}" == "protocol"
        assert format(SymbolKind.PROTOCOL, ">10") == "  protocol"

    def test_json_round_trips_as_a_string(self):
        assert json.dumps({"kind": SymbolKind.ENUM}) == '{"kind": "enum"}'

    def test_value_is_explicit_where_the_wire_form_matters(self):
        assert SymbolKind.DATACLASS.value == "dataclass"


class TestASTSymbol:
    def test_is_hashable_and_frozen(self):
        built = symbol("def f()")
        assert len({built, built}) == 1

    def test_normalisation_survives_the_frozen_decorator(self):
        assert symbol("def f(\n    x: int,\n) -> str").signature == "def f( x: int, ) -> str"

    def test_a_signature_already_normal_is_left_alone(self):
        assert symbol("def f() -> None").signature == "def f() -> None"


class TestSignatureNormalisation:
    """A signature is one row of an index, whichever handler produced it."""

    @pytest.mark.parametrize(
        ("raw", "expected"),
        [
            ("def f()", "def f()"),
            ("def f(\n    x: int,\n)", "def f( x: int, )"),
            ("  def f()  ", "def f()"),
            ("def f(a,\tb)", "def f(a, b)"),
            ("", ""),
        ],
    )
    def test_whitespace_runs_collapse_to_one_space(self, raw, expected):
        assert normalise_signature(raw) == expected

    def test_an_over_long_signature_is_capped_and_marked(self):
        result = normalise_signature("def f(" + "x" * 200 + ")")
        assert len(result) == SIGNATURE_MAX_CHARS
        assert result.endswith("...")

    def test_a_signature_exactly_at_the_cap_is_untouched(self):
        exact = "d" * SIGNATURE_MAX_CHARS
        assert normalise_signature(exact) == exact

    def test_normalising_twice_changes_nothing(self):
        once = normalise_signature("def f(\n" + "y" * 200 + "\n)")
        assert normalise_signature(once) == once

    def test_the_result_never_spans_lines(self):
        assert "\n" not in normalise_signature("class A(\n    B,\n    C,\n)")
