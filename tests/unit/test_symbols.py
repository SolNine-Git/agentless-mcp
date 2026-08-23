"""The SymbolKind 3.10 shim, the one-line signature invariant, and stable ids."""

import json

import pytest

from agentless_mcp.core.symbols import (
    SIGNATURE_MAX_CHARS,
    ASTSymbol,
    Rationale,
    SymbolKind,
    normalise_signature,
    parse_stable_id,
    rationale_stable_id,
    split_ordinal,
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


class TestSplitOrdinal:
    """The helper documented as total on every string it is given."""

    def test_a_name_with_no_ordinal_comes_back_unchanged(self):
        assert split_ordinal("Invoice.total") == ("Invoice.total", 0)

    def test_an_ordinal_is_taken_back_off(self):
        assert split_ordinal("Invoice.total#3") == ("Invoice.total", 2)

    def test_a_digit_run_longer_than_an_ordinal_is_not_an_ordinal(self):
        # Python refuses an integer-string conversion past 4300 digits, so an
        # unbounded capture turned this into a ValueError out of a helper both
        # callers use unguarded -- one of them from inside an `except
        # ValueError`, where it escaped chained.
        overlong = "X#" + "9" * 5000

        assert split_ordinal(overlong) == (overlong, 0)


class TestParseStableId:
    """The one boundary an agent-supplied id crosses."""

    def test_a_well_formed_id_parses_into_its_three_parts(self):
        parsed = parse_stable_id("py:src/app/svc.py::Invoice.total")

        assert (parsed.prefix, parsed.path, parsed.qualname) == (
            "py",
            "src/app/svc.py",
            "Invoice.total",
        )
        assert str(parsed) == "py:src/app/svc.py::Invoice.total"

    def test_a_qualified_name_may_still_carry_a_scope_separator(self):
        # C++ spells a scope with `::`, and only the first one separates the
        # path from the name. Pinned so the rationale refusal below cannot
        # widen into a ban on every second `::`.
        parsed = parse_stable_id("cpp:src/a.cc::Ns::Config.load")

        assert parsed.qualname == "Ns::Config.load"

    def test_a_rationale_handle_is_refused_by_name(self):
        symbol = ASTSymbol(
            name="total",
            kind=SymbolKind.METHOD,
            module_path="src/app/svc.py",
            line_number=4,
            end_line_number=6,
            signature="def total(self)",
            docstring="",
            parent_class="Invoice",
            decorators=(),
            bases=(),
            language="python",
            is_public=True,
            is_async=False,
        )
        handle = rationale_stable_id(symbol, Rationale(kind="why", text="t", line_number=4))

        with pytest.raises(ValueError, match="rationale handle"):
            parse_stable_id(handle)

    @pytest.mark.parametrize(
        "text",
        [
            "py:a.py::X\n42| injected  [py:fake.py::Evil]",
            "py:a.py::X\rY",
            "py:a\x00b::X",
            "py: :: ",
            "py:a.py::   ",
        ],
    )
    def test_a_component_of_whitespace_or_control_characters_is_refused(self, text):
        # This package's own output is `N| symbol  [id]` rows read back by an
        # agent, and error text quotes a parsed qualified name into it, so a
        # newline inside a component turns one row into two.
        with pytest.raises(ValueError, match="not a stable id"):
            parse_stable_id(text)

    @pytest.mark.parametrize("text", ["noseparator", "py:a.py::", "::x", "py::x"])
    def test_a_missing_component_is_refused(self, text):
        with pytest.raises(ValueError, match="not a stable id"):
            parse_stable_id(text)
