"""complexity_delta: D2's three-pass matching, its four cases, and per-file degradation."""

import itertools

import pytest

from agentless_mcp.core import complexity, erosion
from agentless_mcp.core.erosion import TextPair, complexity_delta, pair_language
from agentless_mcp.core.patchlint import Severity
from agentless_mcp.util.errors import LanguageUnavailable

THRESHOLD = 10


def function(name, cc, tag="", indent=""):
    """Python source for one function of complexity ``cc`` and SLOC ``2 * cc``.

    ``tag`` changes the body without changing either number, so two versions of
    a function do not pair in the body pass.
    """
    lines = [f"{indent}def {name}(value):"]
    for index in range(cc - 1):
        lines += [f"{indent}    if value == {index}:", f"{indent}        return {index}"]
    lines.append(f'{indent}    return "{tag}"')
    return "\n".join(lines) + "\n"


def method(owner, name, cc, tag=""):
    """A one-method class whose method has complexity ``cc``."""
    return f"class {owner}:\n" + function(name, cc, tag, indent="    ")


def go_function(name, cc):
    """Go source for one function of complexity ``cc``."""
    branches = "".join(
        f"\tif v == {index} {{\n\t\treturn {index}\n\t}}\n" for index in range(cc - 1)
    )
    return f"func {name}(v int) int {{\n{branches}\treturn -1\n}}\n"


def java_overload(parameter, cc, tag):
    """One overload of `S.go` with complexity ``cc``; ``tag`` changes its body."""
    branches = "".join(f"    if (x == null) return {index};\n" for index in range(cc - 1))
    return f'  int go({parameter} x) {{\n{branches}    return "{tag}".length();\n  }}\n'


def run(old, new, path="app.py", threshold=THRESHOLD):
    return complexity_delta([TextPair(path, old, new)], threshold)


def only(results):
    (found,) = results
    return found


class TestBodyPass:
    def test_an_unchanged_file_has_no_finding(self):
        text = function("big", 15)
        assert run(text, text) == ()

    def test_a_rename_without_other_change_is_paired_by_body(self):
        assert run(function("big", 15), function("renamed", 15)) == ()

    def test_a_unit_moved_within_the_file_is_paired_by_body(self):
        old = function("small", 2) + "\n\n" + function("big", 15)
        new = function("big", 15) + "\n\n" + function("small", 2)
        assert run(old, new) == ()


class TestNamePass:
    def test_a_unit_that_grows_while_over_the_threshold_is_reported(self):
        found = only(run(function("handle", 11), function("handle", 12, tag="x")))

        assert found.finding.check == erosion.CHECK_COMPLEXITY_DELTA
        assert found.finding.severity is Severity.WARNING
        assert found.finding.path == "app.py"
        assert found.finding.line == 1
        assert found.finding.message == (
            "handle: complexity 11 -> 12, 22 -> 24 source lines. This function is already "
            "above the threshold (10), and this change adds more branching to it. Put the new "
            "branches in a new named function and call it, or refactor this function first in "
            "its own change."
        )
        assert found.finding.evidence == "cc 11 -> 12; sloc 22 -> 24; threshold 10"
        assert found.detail is not None
        assert found.detail.as_dict() == {
            "unit": "handle",
            "match": "name",
            "cc_old": 11,
            "cc_new": 12,
            "sloc_old": 22,
            "sloc_new": 24,
            "threshold": 10,
        }

    def test_a_unit_that_crosses_the_threshold_is_reported(self):
        found = only(run(function("handle", 10), function("handle", 11)))

        assert found.finding.message.startswith("handle: complexity 10 -> 11, 20 -> 22 source")
        assert "This change takes the function over the threshold (10)." in found.finding.message
        assert found.finding.evidence == "cc 10 -> 11; sloc 20 -> 22; threshold 10"

    @pytest.mark.parametrize(
        ("old_cc", "new_cc"),
        [(9, 10), (15, 12), (3, 5), (12, 12)],
        ids=["reaches-the-threshold", "falls", "grows-below", "unchanged-cc"],
    )
    def test_no_other_paired_change_is_reported(self, old_cc, new_cc):
        assert run(function("handle", old_cc), function("handle", new_cc, tag="x")) == ()

    def test_a_removed_unit_is_not_reported(self):
        assert run(function("big", 15) + function("kept", 2), function("kept", 2)) == ()


class TestNewUnits:
    def test_a_write_that_creates_a_file_reports_each_unit_over_the_threshold(self):
        found = only(run(None, function("fresh", 12) + function("tiny", 3)))

        assert found.finding.message == (
            "fresh: complexity 12, 24 source lines. This function is new, or moved from another "
            "file, and it is above the threshold (10). Split it into smaller named functions. If "
            "it moved here unchanged, it was already above the threshold in its old file."
        )
        assert found.finding.evidence == "cc 12; sloc 24; threshold 10"
        assert found.detail is not None
        assert found.detail.as_dict() == {
            "unit": "fresh",
            "match": "new",
            "cc_old": None,
            "cc_new": 12,
            "sloc_old": None,
            "sloc_new": 24,
            "threshold": 10,
        }

    def test_an_existing_empty_file_counts_every_unit_as_new(self):
        assert only(run("", function("fresh", 11))).detail.match == "new"

    def test_a_new_unit_at_the_threshold_is_not_reported(self):
        assert run("", function("fresh", 10)) == ()

    def test_a_rename_with_an_added_branch_is_reported_as_new(self):
        found = only(run(function("before", 11), function("after", 12)))
        assert found.detail.unit == "after"
        assert found.detail.match == "new"

    def test_a_write_that_empties_a_file_reports_nothing(self):
        assert run(function("big", 15), "") == ()

    def test_a_deleted_file_reports_nothing(self):
        assert run(function("big", 15), None) == ()


class TestSameNames:
    def test_same_name_methods_in_different_classes_pair_by_qualname(self):
        old = method("A", "go", 12, tag="a") + "\n\n" + method("B", "go", 3, tag="b")
        new = method("A", "go", 12, tag="a") + "\n\n" + method("B", "go", 12, tag="b")

        found = only(run(old, new))

        assert found.detail.unit == "B.go"
        assert found.detail.match == "name"
        assert (found.detail.cc_old, found.detail.cc_new) == (3, 12)

    def test_a_same_name_unit_inserted_above_gives_no_finding(self):
        old = function("handler", 20, tag="v1")
        new = function("handler", 3, tag="added") + function("handler", 20, tag="v2")
        assert run(old, new) == ()

    def test_swapped_same_name_units_give_no_finding(self):
        old = function("handler", 11, tag="a") + function("handler", 20, tag="b")
        new = function("handler", 20, tag="b2") + function("handler", 11, tag="a2")
        assert run(old, new) == ()

    @pytest.mark.parametrize("order", list(itertools.permutations((11, 15, 20))))
    def test_reordered_same_name_units_give_no_finding(self, order):
        old = "".join(function("handler", cc, tag="old") for cc in (11, 15, 20))
        new = "".join(function("handler", cc, tag="new") for cc in order)
        assert run(old, new) == ()

    def test_a_grown_unit_among_same_name_units_is_paired_by_rank(self):
        old = function("handler", 11, tag="a") + function("handler", 20, tag="b")
        new = function("handler", 21, tag="b2") + function("handler", 11, tag="a2")

        found = only(run(old, new))

        assert found.detail.match == "collision"
        assert (found.detail.cc_old, found.detail.cc_new) == (20, 21)
        assert found.finding.line == 1

    def test_swapped_java_overloads_give_no_finding(self):
        old = f"class S {{\n{java_overload('int', 11, 'a')}{java_overload('String', 20, 'b')}}}\n"
        new = f"class S {{\n{java_overload('String', 20, 'c')}{java_overload('int', 11, 'd')}}}\n"
        assert run(old, new, path="S.java") == ()


class TestLineEndings:
    def test_a_crlf_file_reports_the_unit_line(self):
        prefix = '"""Module."""\n\n\n'
        old = (prefix + function("handle", 11)).replace("\n", "\r\n")
        new = (prefix + function("handle", 12)).replace("\n", "\r\n")

        assert only(run(old, new)).finding.line == 4

    def test_a_line_ending_change_alone_is_not_a_finding(self):
        text = function("big", 15)
        assert run(text, text.replace("\n", "\r\n")) == ()


class TestDegradedInput:
    def test_one_unparsable_file_does_not_hide_a_regression_in_another(self):
        results = complexity_delta(
            [
                TextPair("app.py", function("handle", 11), function("handle", 12)),
                TextPair("broken.py", "def f(:\n", "def f(:\n    pass\n"),
            ],
            THRESHOLD,
        )

        regression, gap = results
        assert gap.finding.path == "broken.py"
        assert gap.finding.severity is Severity.NOT_CHECKED
        assert gap.finding.check == erosion.CHECK_COMPLEXITY_DELTA
        assert gap.finding.line == 0
        assert gap.finding.message.startswith(
            "not checked: complexity_delta did not compare broken.py, because its text before "
            "the change cannot be measured. broken.py does not parse"
        )
        assert gap.finding.evidence == "UnmeasurableSource"
        assert gap.detail is None
        assert regression.finding.path == "app.py"
        assert regression.finding.severity is Severity.WARNING

    def test_an_unparsable_new_text_names_the_after_side(self):
        found = only(run(function("handle", 2), "def handle(:\n"))
        assert "because its text after the change cannot be measured" in found.finding.message

    def test_a_tokenizer_rejection_is_not_checked(self):
        rejected = 'LABEL = "left\u2028right"\n\n\ndef f(a):\n    return a or 1\n'
        found = only(run(rejected, rejected + "\n"))
        assert found.finding.severity is Severity.NOT_CHECKED
        assert "tokenizer rejects this file" in found.finding.message

    def test_an_unavailable_grammar_is_not_checked(self, monkeypatch):
        def cold(_source, _path, _language):
            message = "python is not warmed"
            raise LanguageUnavailable(message)

        monkeypatch.setattr(complexity, "measure", cold)

        found = only(run(function("handle", 2), function("handle", 3)))
        assert found.finding.severity is Severity.NOT_CHECKED
        assert found.finding.evidence == "LanguageUnavailable"
        assert "python is not warmed" in found.finding.message

    @pytest.mark.parametrize("path", ["README.md", "data.json", "notes"])
    def test_a_language_without_complexity_rules_gives_no_row(self, path):
        assert run("a", "b", path=path) == ()


class TestScope:
    @pytest.mark.parametrize(
        ("path", "language"),
        [("app.py", "python"), ("src/main.go", "go"), ("README.md", None), ("x.json", None)],
    )
    def test_pair_language_keeps_the_languages_with_complexity_rules(self, path, language):
        assert pair_language(path) == language

    def test_the_threshold_argument_moves_the_result(self):
        old, new = function("handle", 5), function("handle", 6)
        assert run(old, new) == ()
        assert only(run(old, new, threshold=5)).detail.threshold == 5

    def test_a_go_function_that_crosses_the_threshold_is_reported(self):
        prefix = "package main\n\n"
        found = only(
            run(prefix + go_function("Pick", 10), prefix + go_function("Pick", 11), "a.go")
        )
        assert found.detail.unit == "Pick"
        assert found.finding.line == 3

    def test_a_unicode_identifier_is_reported_by_its_name(self):
        found = only(run(function("größe", 11), function("größe", 12)))
        assert found.finding.message.startswith("größe: complexity 11 -> 12")

    def test_findings_come_back_by_path_then_line(self):
        results = complexity_delta(
            [
                TextPair("b.py", None, function("late", 11) + function("early", 12)),
                TextPair("a.py", None, function("only", 13)),
            ],
            THRESHOLD,
        )
        assert [(found.finding.path, found.finding.line) for found in results] == [
            ("a.py", 1),
            ("b.py", 1),
            ("b.py", 23),
        ]
