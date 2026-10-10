"""clone_delta: D4's count rule, moves, nesting, the near_duplicates overlap, and coverage rows."""

import pytest

from agentless_mcp.core import erosion
from agentless_mcp.core.clones import clone_groups, file_blocks
from agentless_mcp.core.erosion import CloneIndex, TextPair, clone_delta, clone_index
from agentless_mcp.core.patchlint import Severity
from agentless_mcp.util.errors import UnmeasurableSource


def total(name="total", items="items", start="0"):
    """A five-line function of 71 leaf tokens, with a two-line loop inside it."""
    return (
        f"def {name}({items}, scale=1, offset=None):\n"
        f"    out = {start} * scale + len({items}) - (offset or 0)\n"
        f"    for item in sorted({items}, key=abs, reverse=True):\n"
        f"        out += item * scale - {start} + max(item, 0)\n"
        f"    return round(out, 2) if out else {start}\n"
    )


LOOP = (
    "for row in sorted(rows, key=len, reverse=True)[:limit]:\n"
    "    print(row, sep=', ', end='', flush=True)\n"
    "    found += len(row) * limit - min(len(row), limit)\n"
)


def looped(name="scan", seed="0"):
    """A function whose three-line loop of 54 leaf tokens is a candidate of its own."""
    body = "".join(f"    {line}\n" for line in LOOP.splitlines())
    return f"def {name}(rows, limit=10):\n    found = {seed}\n{body}    return found\n"


def repo(files, unread=None):
    return clone_index(files, unread or {})


def run(pairs, files=None, reported=frozenset(), unread=None):
    return clone_delta(pairs, repo(files or {}, unread), reported).findings


def copies(results):
    return [found.finding for found in results if found.finding.severity is Severity.ADVISORY]


def gaps(results):
    return [found.finding for found in results if found.finding.severity is Severity.NOT_CHECKED]


ORIGINAL = {"a.py": total()}


class TestNewCopies:
    def test_a_new_copy_in_another_file_is_an_advisory_at_the_copy(self):
        (found,) = copies(run([TextPair("b.py", None, total("again"))], ORIGINAL))

        assert found.check == erosion.CHECK_CLONE_DELTA
        assert (found.path, found.line) == ("b.py", 1)
        assert found.evidence == "also at a.py:1"
        assert found.message.startswith("lines 1-5 add a copy of a block")
        assert "now holds 2 copies" in found.message

    def test_a_renamed_identifier_copy_is_a_finding(self):
        copy = total("summed", "values")
        assert len(copies(run([TextPair("b.py", "", copy)], ORIGINAL))) == 1

    def test_a_literal_changed_copy_is_a_finding(self):
        copy = total("again", start="10")
        assert len(copies(run([TextPair("b.py", "", copy)], ORIGINAL))) == 1

    def test_a_copy_in_the_same_file_is_reported_at_the_copy(self):
        old = total()
        new = old + "\n\n" + total("again")

        (found,) = copies(run([TextPair("a.py", old, new)], {"a.py": old}))

        assert (found.path, found.line) == ("a.py", 8)

    def test_a_renamed_copy_inserted_above_its_original_is_reported_at_the_copy(self):
        old = total()
        new = total("again") + "\n\n" + old

        (found,) = copies(run([TextPair("a.py", old, new)], {"a.py": old}))

        assert found.line == 1

    @pytest.mark.parametrize(
        ("new", "line"),
        [
            (looped() + "\n\n" + total() + "\n\n" + total(), 16),
            (total() + "\n\n" + looped() + "\n\n" + total(), 1),
        ],
        ids=["below", "above"],
    )
    def test_an_exact_copy_is_the_block_the_diff_did_not_keep_in_place(self, new, line):
        old = looped() + "\n\n" + total()

        (found,) = copies(run([TextPair("a.py", old, new)], {"a.py": old}))

        assert found.line == line

    def test_two_new_copies_of_one_new_block_are_two_findings(self):
        pairs = [TextPair("b.py", None, total("one")), TextPair("c.py", None, total("two"))]

        found = copies(run(pairs))

        assert [(row.path, row.line) for row in found] == [("b.py", 1), ("c.py", 1)]
        assert found[0].evidence == "also at c.py:1"

    def test_a_third_copy_names_both_others(self):
        files = {"a.py": total(), "b.py": total("again")}

        (found,) = copies(run([TextPair("c.py", None, total("third"))], files))

        assert found.evidence == "also at a.py:1, b.py:1"
        assert "now holds 3 copies" in found.message

    def test_more_than_three_other_copies_are_counted(self):
        files = {f"{name}.py": total(name) for name in ("a", "b", "c", "d")}

        (found,) = copies(run([TextPair("e.py", None, total("e"))], files))

        assert found.evidence == "also at a.py:1, b.py:1, c.py:1 and 1 more"


class TestNotFindings:
    def test_a_new_block_with_no_twin_is_not_a_finding(self):
        assert run([TextPair("b.py", None, looped())], ORIGINAL) == ()

    def test_an_unchanged_file_is_not_a_finding(self):
        assert run([TextPair("a.py", total(), total())], ORIGINAL) == ()

    def test_moving_one_of_two_instances_to_another_file_is_not_a_finding(self):
        files = {"a.py": total(), "b.py": total("again")}
        pairs = [TextPair("b.py", total("again"), ""), TextPair("c.py", None, total("again"))]

        assert run(pairs, files) == ()

    def test_renaming_one_of_two_instances_in_place_is_not_a_finding(self):
        files = {"a.py": total(), "b.py": total("again")}

        assert run([TextPair("b.py", total("again"), total("renamed"))], files) == ()

    def test_a_copy_of_a_block_under_three_source_lines_is_not_a_finding(self):
        short = "def f(x):\n    return x + 1\n"

        assert run([TextPair("b.py", None, short.replace("f(", "g("))], {"a.py": short}) == ()

    def test_a_copied_type_checking_block_is_not_a_finding(self):
        block = "if TYPE_CHECKING:\n    import os\n    import sys\n    import re\n"

        assert run([TextPair("b.py", None, block)], {"a.py": block}) == ()

    def test_a_copied_function_inside_a_type_checking_block_is_a_finding(self):
        block = "if TYPE_CHECKING:\n" + "".join(f"    {line}\n" for line in total().splitlines())

        (found,) = copies(run([TextPair("b.py", None, block)], {"a.py": total()}))

        assert found.line == 2

    def test_deleting_a_copy_is_not_a_finding(self):
        files = {"a.py": total(), "b.py": total("again")}

        assert run([TextPair("b.py", total("again"), None)], files) == ()

    def test_a_deleted_instance_leaves_the_count(self):
        files = {"a.py": total(), "b.py": total("again")}
        pairs = [TextPair("b.py", total("again"), None), TextPair("c.py", None, total("third"))]

        assert run(pairs, files) == ()


def padded(passes):
    """Six five-token lines, ``passes`` one-token lines, and a header and return of 13."""
    return "def f(a):\n" + "    a = a + 1\n" * 6 + "    pass\n" * passes + "    return a\n"


GUARD = (
    "def f(value):\n"
    "    if value is None:\n"
    '        message = "value is required"\n'
    "        raise ValueError(message)\n"
    "    return value\n"
)


class TestTheTokenFloor:
    def test_a_copy_at_the_floor_is_a_finding(self):
        source = padded(2)
        (block,) = file_blocks(source, "a.py", "python").blocks

        found = copies(run([TextPair("b.py", None, source.replace("f(", "g("))], {"a.py": source}))

        assert block.tokens == erosion.MIN_FINDING_TOKENS
        assert len(found) == 1

    def test_a_copy_one_token_under_the_floor_is_not_a_finding(self):
        source = padded(1)
        (block,) = file_blocks(source, "a.py", "python").blocks

        assert block.tokens == erosion.MIN_FINDING_TOKENS - 1
        assert run([TextPair("b.py", None, source.replace("f(", "g("))], {"a.py": source}) == ()

    def test_a_copied_guard_clause_is_not_a_finding(self):
        copy = GUARD.replace("f(", "g(").replace("value is required", "missing")

        assert run([TextPair("b.py", None, copy)], {"a.py": GUARD}) == ()

    def test_the_floor_leaves_the_groups_alone(self):
        copy = GUARD.replace("f(", "g(")
        files = [file_blocks(GUARD, "a.py", "python"), file_blocks(copy, "b.py", "python")]

        groups = clone_groups(files)

        assert [len(group.instances) for group in groups] == [2, 2]
        assert all(block.tokens < erosion.MIN_FINDING_TOKENS for block in files[0].blocks)


class TestNesting:
    def test_a_copied_function_with_a_copied_loop_is_one_finding(self):
        (found,) = copies(run([TextPair("b.py", None, looped("again"))], {"a.py": looped()}))

        assert found.line == 1

    def test_a_copied_loop_in_a_new_function_is_reported_at_the_loop(self):
        body = "".join(f"    {line}\n" for line in LOOP.splitlines())
        head = "def fresh(rows, extra):\n    found = extra * 2\n"
        new = f"{head}{body}    print(extra)\n    return found\n"

        (found,) = copies(run([TextPair("b.py", None, new)], {"a.py": looped()}))

        assert found.line == 3


class TestTheNearDuplicatesOverlap:
    def test_a_function_near_duplicates_reported_is_skipped(self):
        reported = frozenset({("b.py", 1)})

        assert run([TextPair("b.py", None, total())], ORIGINAL, reported) == ()

    def test_a_block_nested_in_a_skipped_function_is_skipped_too(self):
        reported = frozenset({("b.py", 1)})

        assert run([TextPair("b.py", None, looped("again"))], {"a.py": looped()}, reported) == ()

    def test_a_loop_at_a_reported_line_is_not_skipped(self):
        reported = frozenset({("b.py", 1)})

        (found,) = copies(run([TextPair("b.py", None, LOOP)], {"a.py": LOOP}, reported))

        assert found.line == 1

    def test_another_line_is_not_skipped(self):
        reported = frozenset({("b.py", 7)})

        assert len(copies(run([TextPair("b.py", None, total())], ORIGINAL, reported))) == 1


class TestCoverage:
    def test_an_unparsable_index_file_gives_one_row_and_the_copy_is_still_reported(self):
        files = {**ORIGINAL, "broken.py": "def f(:\n"}

        results = run([TextPair("b.py", None, total("again"))], files)

        (row,) = gaps(results)
        assert row.check == erosion.CHECK_CLONE_DELTA
        assert row.location == "(repository)"
        assert row.message.startswith(
            "not checked: clone_delta did not index 1 of 2 repository files, so it does not see"
        )
        assert row.message.endswith(": broken.py.")
        assert row.evidence.startswith("first: broken.py: broken.py does not parse")
        assert len(copies(results)) == 1

    def test_an_unread_index_file_is_named_with_its_reason(self):
        results = run([TextPair("b.py", None, total())], ORIGINAL, unread={"big.py": "skipped: x"})

        (row,) = gaps(results)
        assert row.evidence == "first: big.py: skipped: x"

    def test_an_unindexed_file_the_run_changes_gets_only_its_pair_row(self):
        files = {**ORIGINAL, "broken.py": "def f(:\n"}

        (row,) = gaps(run([TextPair("broken.py", "def f(:\n", total("x"))], files))

        assert row.path == "broken.py"
        assert "before the change cannot be indexed" in row.message

    def test_more_than_ten_unindexed_files_are_counted(self):
        broken = {f"b{index:02}.py": "def f(:\n" for index in range(12)}

        (row,) = gaps(run([TextPair("c.py", None, total())], broken))

        assert row.message.endswith("b08.py, b09.py and 2 more.")

    def test_an_unparsable_pair_side_gives_one_row_for_that_file(self):
        results = run([TextPair("b.py", "def f(:\n", total("again"))], ORIGINAL)

        (row,) = gaps(results)
        assert (row.path, row.evidence) == ("b.py", "UnmeasurableSource")
        assert row.message.startswith("not checked: clone_delta did not compare b.py, because its")
        assert "before the change cannot be indexed" in row.message
        assert copies(results) == []

    def test_an_unparsable_new_side_names_the_after_side(self):
        (row,) = gaps(run([TextPair("b.py", total(), "def f(:\n")], ORIGINAL))

        assert "after the change cannot be indexed" in row.message

    def test_a_repository_that_could_not_be_listed_gives_one_row(self):
        index = CloneIndex(unlisted="walk refused: too many files")
        pairs = [TextPair("b.py", None, total("one")), TextPair("c.py", None, total("two"))]

        results = clone_delta(pairs, index, frozenset()).findings

        (row,) = gaps(results)
        assert row.message == (
            "not checked: clone_delta could not list the repository files (walk refused: too many "
            "files), so it compared the changed files only with each other."
        )
        assert len(copies(results)) == 2

    @pytest.mark.parametrize("path", ["notes.md", "data.json"])
    def test_a_pair_in_a_language_with_no_table_gives_no_row(self, path):
        assert run([TextPair(path, None, "x")], ORIGINAL) == ()

    def test_one_path_twice_in_a_run_is_refused(self):
        with pytest.raises(ValueError, match="only one text pair"):
            run([TextPair("b.py", None, "x = 1\n"), TextPair("b.py", None, "y = 1\n")])

    def test_the_index_refuses_a_file_that_is_not_tier1_source(self):
        with pytest.raises(ValueError, match="not a tier-1 source file"):
            clone_index({"notes.md": "# x"}, {})

    def test_the_index_reads_each_text_through_the_reader_it_is_given(self):
        seen = []

        def reader(text, path, language):
            seen.append((path, language))
            if path == "b.py":
                message = "b.py: the reader has no blocks for it"
                raise UnmeasurableSource(message)
            return file_blocks(text, path, language)

        index = clone_index({"a.py": total(), "b.py": total()}, {}, reader)

        assert seen == [("a.py", "python"), ("b.py", "python")]
        assert set(index.files) == {"a.py"}
        assert dict(index.unindexed) == {"b.py": "b.py: the reader has no blocks for it"}


def function_key(source, path="b.py"):
    (block,) = file_blocks(source, path, "python").blocks
    return block.key


class TestDetail:
    def test_a_finding_carries_its_key_its_span_and_its_other_sites(self):
        ((found, detail),) = [
            (row.finding, row.detail)
            for row in clone_delta(
                [TextPair("b.py", None, total())], repo(ORIGINAL), frozenset()
            ).findings
        ]

        assert detail.as_dict() == {
            "key": function_key(total()),
            "line": found.line,
            "end_line": 5,
            "sites": [{"path": "a.py", "line": 1, "end_line": 5}],
            "sites_total": 1,
        }

    def test_the_detail_lists_twenty_sites_and_counts_them_all(self):
        files = {f"c{index:02}.py": total() for index in range(25)}

        (row,) = clone_delta([TextPair("b.py", None, total())], repo(files), frozenset()).findings

        detail = row.detail.as_dict()
        assert erosion.MAX_CLONE_SITES == 20
        assert [site["path"] for site in detail["sites"]] == sorted(files)[:20]
        assert detail["sites_total"] == 25


class TestObserved:
    def test_a_reportable_block_with_a_copy_is_observed(self):
        outcome = clone_delta([TextPair("b.py", None, total())], repo(ORIGINAL), frozenset())

        assert outcome.observed.as_dict() == {
            "clone_instances": [
                {"path": "b.py", "key": function_key(total()), "line": 1, "end_line": 5}
            ]
        }

    def test_the_original_and_its_new_copy_in_one_text_are_both_observed(self):
        pair = TextPair("a.py", total(), total() + "\n\n" + total("again"))

        outcome = clone_delta([pair], repo(ORIGINAL), frozenset())

        rows = outcome.observed.as_dict()["clone_instances"]
        assert [(row["line"], row["end_line"]) for row in rows] == [(1, 5), (8, 12)]

    def test_a_block_with_no_copy_is_not_observed(self):
        outcome = clone_delta([TextPair("b.py", None, looped())], repo(ORIGINAL), frozenset())

        assert outcome.observed.as_dict() == {"clone_instances": []}

    def test_a_copy_under_the_token_floor_is_not_observed(self):
        guard = "if not rows:\n    log(rows)\n    return\n"
        index = repo({"a.py": guard})

        outcome = clone_delta([TextPair("b.py", None, guard)], index, frozenset())

        assert outcome.findings == ()
        assert outcome.observed.as_dict() == {"clone_instances": []}

    def test_a_run_that_indexed_no_pair_observes_nothing(self):
        outcome = clone_delta([TextPair("README.md", None, "x")], repo(ORIGINAL), frozenset())

        assert outcome.observed.as_dict() == {}

    def test_a_deleted_file_is_observed_with_nothing_in_its_new_text(self):
        outcome = clone_delta([TextPair("a.py", total(), None)], repo(ORIGINAL), frozenset())

        assert outcome.observed.as_dict() == {"clone_instances": []}


class TestOtherLanguages:
    def test_a_go_copy_is_a_finding(self):
        source = (
            "func Sum(a int, b int) int {\n\tc := a + b*2 - len(names)\n"
            '\tfmt.Println("x", a, b, c, names[0])\n\treturn c * a / (b + 1)\n}\n'
        )
        files = {"a.go": "package a\n\n" + source}
        copy = "package a\n\n" + source.replace("Sum", "Add").replace('"x"', '"y"')

        (found,) = copies(run([TextPair("b.go", None, copy)], files))

        assert (found.path, found.line) == ("b.go", 3)

    def test_a_crlf_file_reports_the_copy_line(self):
        copy = ("import os\n\n\n" + total("again")).replace("\n", "\r\n")

        (found,) = copies(run([TextPair("b.py", None, copy)], ORIGINAL))

        assert found.line == 4
