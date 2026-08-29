"""Characterization of the Agentless location grammar, ported to symbols.

Each test names the case in ``transfer_arb_locs_to_locs`` it transcribes. The
implicit cases -- the current-class fallback, the dotted ``class:`` falling
through to the method branch, the any-class search accepted only when unique
-- are behaviour that lived in the control flow rather than in any docstring,
so they are pinned here rather than re-derived.
"""

from dataclasses import replace

from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.locs import LocTarget, resolve_locs
from agentless_mcp.core.slices import line_count

SOURCE = '''\
"""Module."""

MAX_ITEMS = 500


class Invoice:
    def total(self):
        return 1

    def render(self):
        return ""


class Receipt:
    def total(self):
        return 2

    def stamp(self):
        return ""


def format_money(amount):
    return str(amount)
'''


def target(path="billing.py"):
    """Build the LocTarget the tests resolve against."""
    extractor = TreeSitterExtractor()
    symbols = extractor.extract_from_source(SOURCE, "python", path)
    return LocTarget(
        path=path,
        language="python",
        symbols=tuple(symbols),
        total_lines=line_count(SOURCE),
    )


class TestClassLocations:
    def test_a_class_resolves_to_its_span_and_id(self):
        result = resolve_locs(["class: Invoice"], target())
        assert result.stable_ids == ("py:billing.py::Invoice",)
        assert result.spans == ((6, 11),)

    def test_an_unknown_class_is_reported_not_dropped(self):
        result = resolve_locs(["class: Invoic"], target())
        assert result.spans == ()
        assert result.unrecognized[0].loc == "class: Invoic"
        assert "no class named 'Invoic'" in result.unrecognized[0].reason

    def test_a_dotted_class_location_is_treated_as_a_method(self):
        """The original's first branch requires no dot, so `class: A.b` falls
        through to the function branch. Model output uses both spellings."""
        result = resolve_locs(["class: Invoice.total"], target())
        assert result.stable_ids == ("py:billing.py::Invoice.total",)


class TestFunctionLocations:
    def test_a_module_function_resolves(self):
        result = resolve_locs(["function: format_money"], target())
        assert result.stable_ids == ("py:billing.py::format_money",)

    def test_a_qualified_method_resolves(self):
        result = resolve_locs(["function: Receipt.stamp"], target())
        assert result.stable_ids == ("py:billing.py::Receipt.stamp",)

    def test_a_bare_qualified_name_resolves_without_a_prefix(self):
        result = resolve_locs(["Receipt.stamp"], target())
        assert result.stable_ids == ("py:billing.py::Receipt.stamp",)

    def test_the_current_class_carries_to_the_next_location(self):
        """The state that makes a bare `function:` after a `class:` a method."""
        result = resolve_locs(["class: Receipt", "function: total"], target())
        assert result.stable_ids == (
            "py:billing.py::Receipt",
            "py:billing.py::Receipt.total",
        )

    def test_without_a_current_class_a_unique_method_name_still_resolves(self):
        result = resolve_locs(["function: stamp"], target())
        assert result.stable_ids == ("py:billing.py::Receipt.stamp",)

    def test_an_ambiguous_bare_method_is_reported_not_silently_dropped(self):
        """The original fell through without a trace when several classes
        defined the name. An ambiguous location is a question, not a guess."""
        result = resolve_locs(["function: total"], target())
        assert result.spans == ()
        assert "ambiguous" in result.unrecognized[0].reason
        assert "Invoice" in result.unrecognized[0].reason
        assert "Receipt" in result.unrecognized[0].reason

    def test_a_method_missing_from_a_known_class_names_the_class(self):
        result = resolve_locs(["function: Invoice.stamp"], target())
        assert "class 'Invoice' has no member named 'stamp'" in result.unrecognized[0].reason

    def test_a_missing_name_after_a_class_does_not_blame_that_class(self):
        """The current-class fallback's miss must not leak a reason blaming a
        class the location never mentioned."""
        result = resolve_locs(
            ["function: format_money", "class: Invoice", "line: 7", "function: does_not_exist"],
            target(),
        )
        assert result.unrecognized[0].loc == "function: does_not_exist"
        assert "no function or method named 'does_not_exist'" in result.unrecognized[0].reason
        assert "Invoice" not in result.unrecognized[0].reason
        assert "Receipt" not in result.unrecognized[0].reason

    def test_a_missing_name_without_a_current_class_is_reported_the_same_way(self):
        result = resolve_locs(["function: does_not_exist"], target())
        assert "no function or method named 'does_not_exist'" in result.unrecognized[0].reason

    def test_a_dotted_miss_still_blames_the_class_it_names(self):
        """A dotted location asks for a member of the named class, so the
        reason naming that class stays correct even with a current class set."""
        result = resolve_locs(["class: Invoice", "function: Receipt.nope"], target())
        assert "class 'Receipt' has no member named 'nope'" in result.unrecognized[0].reason

    def test_a_current_class_miss_falls_through_to_the_unique_method_search(self):
        """The docstring's chain: module function, then the current class's
        method, then a method of any class when exactly one defines it."""
        result = resolve_locs(["class: Invoice", "function: stamp"], target())
        assert result.stable_ids == (
            "py:billing.py::Invoice",
            "py:billing.py::Receipt.stamp",
        )


NESTED_SOURCE = '''\
"""Module."""


def outer():
    def inner(x):
        return x

    return inner
'''


class TestNestedFunctionLocations:
    """A dotted function location is not always ``Class.method``.

    A function nested in a function carries its enclosing chain as the
    parent, and the published id pattern qualifies it exactly that way --
    ``outer.inner`` -- so the spelling an id hands back must resolve here
    rather than be refused for naming a class that does not exist.
    """

    def test_a_nested_function_resolves_by_its_published_qualified_name(self):
        extractor = TreeSitterExtractor()
        symbols = extractor.extract_from_source(NESTED_SOURCE, "python", "nest.py")
        nested_target = LocTarget(
            path="nest.py",
            language="python",
            symbols=tuple(symbols),
            total_lines=line_count(NESTED_SOURCE),
        )

        result = resolve_locs(["function: outer.inner"], nested_target)

        assert result.stable_ids == ("py:nest.py::outer.inner",)
        assert result.unrecognized == ()


class TestLineAndVariableLocations:
    def test_a_line_resolves_to_a_single_line_span(self):
        result = resolve_locs(["line: 7"], target())
        assert result.spans == ((7, 7),)

    def test_trailing_text_after_the_number_is_ignored(self):
        result = resolve_locs(["line: 7 in Invoice.total"], target())
        assert result.spans == ((7, 7),)

    def test_a_non_numeric_line_is_reported(self):
        result = resolve_locs(["line: seven"], target())
        assert "not a line number" in result.unrecognized[0].reason

    def test_a_line_past_the_end_of_the_file_is_reported(self):
        result = resolve_locs(["line: 9000"], target())
        assert "outside" in result.unrecognized[0].reason

    def test_a_module_constant_resolves(self):
        result = resolve_locs(["variable: MAX_ITEMS"], target())
        assert result.stable_ids == ("py:billing.py::MAX_ITEMS",)


class TestIntervalsAndInput:
    def test_intervals_widen_by_the_context_window_and_merge(self):
        result = resolve_locs(["line: 7", "line: 10"], target(), context=2)
        assert result.intervals == ((5, 12),)

    def test_intervals_are_clamped_to_the_file_and_are_one_based(self):
        result = resolve_locs(["line: 1"], target(), context=10)
        assert result.intervals == ((1, 11),)

    def test_a_multi_line_block_is_one_location_per_line(self):
        result = resolve_locs("class: Invoice\nfunction: render", target())
        assert len(result.stable_ids) == 2

    def test_an_unknown_form_is_reported_with_the_accepted_ones(self):
        result = resolve_locs(["module: billing"], target())
        assert "expected class:, function:, line: or variable:" in result.unrecognized[0].reason

    def test_nothing_resolvable_yields_empty_intervals_not_the_whole_file(self):
        result = resolve_locs(["class: Nope"], target())
        assert result.intervals == ()


class TestSpansThatNoLongerFitTheFile:
    """A symbol whose line numbers outlived the file on disk.

    Only a cache row written before an edit produces one, and the widened
    interval used to be clamped at the high end alone -- so a symbol at lines
    900-910 of a 30-line file became the interval (890, 30), which the
    renderer answered with all thirty lines as though they were the request.
    """

    def test_a_span_past_the_end_yields_no_interval(self):
        stale = replace(target(), total_lines=5)
        result = resolve_locs(["class: Invoice"], stale, context=0)

        assert result.spans == ((6, 11),)
        assert result.intervals == ()

    def test_a_symbol_with_no_end_line_spans_its_own_line_only(self):
        known = target()
        stale = replace(
            known,
            symbols=tuple(
                replace(symbol, end_line_number=None) if symbol.name == "Invoice" else symbol
                for symbol in known.symbols
            ),
        )
        result = resolve_locs(["class: Invoice"], stale, context=0)

        assert result.spans == ((6, 6),)


class TestAnEmptyFileIsAKnownLineCount:
    """Zero lines is a count, not a missing one.

    The bounds check was guarded on the truthiness of ``total_lines``, so an
    empty ``__init__.py`` disabled it entirely: ``line: 9999`` came back
    resolved with a span of (9999, 9999), and the render that followed raised
    on an interval no line of the file satisfies.
    """

    def test_a_line_past_the_end_of_an_empty_file_is_unrecognized(self):
        empty = replace(target(), symbols=(), total_lines=0)
        result = resolve_locs(["line: 9999"], empty)

        assert result.spans == ()
        assert result.intervals == ()
        assert [entry.reason for entry in result.unrecognized] == [
            "line 9999 is outside billing.py (1-0)"
        ]

    def test_an_unknown_total_still_leaves_the_high_end_unclamped(self):
        unknown = replace(target(), total_lines=None)
        result = resolve_locs(["line: 9999"], unknown, context=0)

        assert result.spans == ((9999, 9999),)
        assert result.intervals == ((9999, 9999),)
        assert result.unrecognized == ()
