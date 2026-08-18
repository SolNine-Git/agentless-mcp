"""Characterization of the SEARCH/REPLACE format: parsing and applying.

The parse cases are a corpus rather than a set of hand-written tests because
the format's variations are data: fenced or bare, heading or plain path, one
file or several, and each of the malformed shapes a truncated or confused
generation produces. A new variation is a row here, not a new test function.

The apply cases pin the three fixes over the Agentless original -- whole-file
search when no intervals are given (which raised ``NameError`` there),
ambiguity refused with a count, and a structured reason on every block that
did not land -- plus the two behaviours that were right and had to survive the
rewrite: whole-line matching and ``...`` elisions.
"""

import pytest

from agentless_mcp.core.patches import (
    Edit,
    EditStatus,
    apply_edits,
    parse_blocks,
)

FENCED = """\
Here is the fix.

```python
### src/app.py
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
```
"""

BARE = """\
### src/app.py
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
"""

NO_HEADING_MARKER = """\
src/app.py
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
"""

MULTI_FILE = """\
```python
### src/app.py
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
```

```python
### src/util.py
<<<<<<< SEARCH
VERSION = "1"
=======
VERSION = "2"
>>>>>>> REPLACE
```
"""

TWO_BLOCKS_ONE_FILE = """\
### src/app.py
<<<<<<< SEARCH
first
=======
FIRST
>>>>>>> REPLACE

<<<<<<< SEARCH
second
=======
SECOND
>>>>>>> REPLACE
"""

FILENAME_WITH_SPACES = """\
### my dir/my file.py
<<<<<<< SEARCH
a
=======
b
>>>>>>> REPLACE
"""

ELIDED = """\
### src/app.py
<<<<<<< SEARCH
...
=======
import sys
>>>>>>> REPLACE
"""

MISSING_DIVIDER = """\
### src/app.py
<<<<<<< SEARCH
    return total
    return round(total, 2)
>>>>>>> REPLACE
"""

TWO_DIVIDERS = """\
### src/app.py
<<<<<<< SEARCH
a
=======
b
=======
c
>>>>>>> REPLACE
"""

NO_SEARCH_MARKER = """\
### src/app.py
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
"""

UNTERMINATED = """\
### src/app.py
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
"""

NO_PATH_ANYWHERE = """\
<<<<<<< SEARCH
a
=======
b
>>>>>>> REPLACE
"""


class TestParseCorpus:
    """One row per shape the format shows up in."""

    @pytest.mark.parametrize(
        ("name", "text", "expected"),
        [
            ("fenced", FENCED, [("src/app.py", "    return total")]),
            ("bare", BARE, [("src/app.py", "    return total")]),
            ("no_heading_hashes", NO_HEADING_MARKER, [("src/app.py", "    return total")]),
            (
                "multi_file",
                MULTI_FILE,
                [("src/app.py", "    return total"), ("src/util.py", 'VERSION = "1"')],
            ),
            (
                "two_blocks_one_file",
                TWO_BLOCKS_ONE_FILE,
                [("src/app.py", "first"), ("src/app.py", "second")],
            ),
            ("filename_with_spaces", FILENAME_WITH_SPACES, [("my dir/my file.py", "a")]),
            ("elided_search", ELIDED, [("src/app.py", "...")]),
        ],
    )
    def test_well_formed_blocks_parse(self, name, text, expected):
        result = parse_blocks(text)
        assert result.errors == (), name
        assert [(edit.path, edit.search) for edit in result.edits] == expected, name

    @pytest.mark.parametrize(
        ("name", "text", "fragment"),
        [
            ("missing_divider", MISSING_DIVIDER, "no ======= divider"),
            ("two_dividers", TWO_DIVIDERS, "2 ======= dividers"),
            ("no_search_marker", NO_SEARCH_MARKER, "no <<<<<<< SEARCH marker"),
            ("unterminated", UNTERMINATED, "not terminated by >>>>>>> REPLACE"),
            ("no_path", NO_PATH_ANYWHERE, "names no file"),
        ],
    )
    def test_malformed_blocks_are_reported_not_dropped(self, name, text, fragment):
        result = parse_blocks(text)
        assert result.edits == (), name
        assert len(result.errors) == 1, name
        assert fragment in result.errors[0].reason, name

    def test_replacement_side_is_taken_verbatim(self):
        (edit,) = parse_blocks(FENCED).edits
        assert edit.replace == "    return round(total, 2)"

    def test_a_malformed_block_does_not_hide_the_good_one(self):
        result = parse_blocks(BARE + "\n" + MISSING_DIVIDER)
        assert len(result.edits) == 1
        assert len(result.errors) == 1
        assert result.errors[0].path == "src/app.py"

    def test_any_fence_label_is_accepted(self):
        text = BARE.replace("### src/app.py", "```typescript\n### src/app.py")
        (edit,) = parse_blocks(text).edits
        assert edit.path == "src/app.py"


SOURCE = "alpha\nbravo\ncharlie\n"


def edit(path: str, search: str, replace: str, index: int = 0) -> Edit:
    """Build one edit without going through the parser."""
    return Edit(index=index, path=path, search=search, replace=replace)


class TestApply:
    def test_a_matching_edit_applies(self):
        result = apply_edits([edit("a.py", "bravo", "BRAVO")], {"a.py": SOURCE})
        assert result.ok
        assert result.new_contents == {"a.py": "alpha\nBRAVO\ncharlie\n"}

    def test_a_missing_search_reports_not_found(self):
        result = apply_edits([edit("a.py", "delta", "DELTA")], {"a.py": SOURCE})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.NOT_FOUND
        assert outcome.reason == "search text not found"
        assert result.new_contents == {}

    def test_an_ambiguous_search_reports_the_count_and_applies_nowhere(self):
        content = "x = 1\ny = 2\nx = 1\n"
        result = apply_edits([edit("a.py", "x = 1", "x = 9")], {"a.py": content})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.AMBIGUOUS
        assert outcome.reason == "search text ambiguous (2 matches)"
        assert outcome.matches == 2
        assert result.new_contents == {}

    def test_adjacent_identical_lines_count_as_two_matches(self):
        """Overlapping matches share a newline; a naive scan would see one."""
        result = apply_edits([edit("a.py", "dup", "DUP")], {"a.py": "dup\ndup\n"})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.AMBIGUOUS
        assert outcome.matches == 2

    def test_matching_is_whole_line_only(self):
        result = apply_edits([edit("a.py", "rav", "RAV")], {"a.py": SOURCE})
        assert result.outcomes[0].status is EditStatus.NOT_FOUND

    def test_an_unknown_path_reports_no_such_file(self):
        result = apply_edits([edit("gone.py", "alpha", "ALPHA")], {"a.py": SOURCE})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.NO_SUCH_FILE
        assert "gone.py" in outcome.reason

    def test_edits_to_one_file_apply_in_order(self):
        edits = [edit("a.py", "alpha", "ALPHA", 0), edit("a.py", "charlie", "CHARLIE", 1)]
        result = apply_edits(edits, {"a.py": SOURCE})
        assert result.ok
        assert result.new_contents == {"a.py": "ALPHA\nbravo\nCHARLIE\n"}

    def test_a_later_edit_sees_the_earlier_one(self):
        edits = [edit("a.py", "bravo", "zulu", 0), edit("a.py", "zulu", "omega", 1)]
        result = apply_edits(edits, {"a.py": SOURCE})
        assert result.ok
        assert result.new_contents == {"a.py": "alpha\nomega\ncharlie\n"}

    def test_a_multi_line_search_applies(self):
        result = apply_edits([edit("a.py", "alpha\nbravo", "one\ntwo\nthree")], {"a.py": SOURCE})
        assert result.new_contents == {"a.py": "one\ntwo\nthree\ncharlie\n"}

    def test_an_edit_at_the_first_line_applies(self):
        result = apply_edits([edit("a.py", "alpha", "ALPHA")], {"a.py": SOURCE})
        assert result.new_contents == {"a.py": "ALPHA\nbravo\ncharlie\n"}

    def test_a_file_with_no_trailing_newline_still_matches_its_last_line(self):
        result = apply_edits([edit("a.py", "charlie", "CHARLIE")], {"a.py": "alpha\ncharlie"})
        assert result.new_contents == {"a.py": "alpha\nCHARLIE"}

    def test_a_file_nothing_applied_to_is_absent_from_new_contents(self):
        result = apply_edits([edit("a.py", "nope", "NOPE")], {"a.py": SOURCE})
        assert result.new_contents == {}
        assert not result.ok


LONG_SOURCE = "one\ntwo\nthree\nfour\ntarget\nsix\n"


class TestIntervals:
    def test_an_edit_inside_its_interval_applies(self):
        result = apply_edits(
            [edit("a.py", "target", "TARGET")],
            {"a.py": LONG_SOURCE},
            intervals={"a.py": [(4, 6)]},
        )
        assert result.ok
        assert result.new_contents == {"a.py": "one\ntwo\nthree\nfour\nTARGET\nsix\n"}

    def test_an_edit_outside_its_interval_is_refused_with_that_reason(self):
        result = apply_edits(
            [edit("a.py", "target", "TARGET")],
            {"a.py": LONG_SOURCE},
            intervals={"a.py": [(1, 3)]},
        )
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.OUTSIDE_INTERVALS
        assert "scoped to" in outcome.reason
        assert result.new_contents == {}

    def test_an_interval_disambiguates_a_repeated_line(self):
        content = "x\nkeep\nx\n"
        result = apply_edits(
            [edit("a.py", "x", "X")], {"a.py": content}, intervals={"a.py": [(3, 3)]}
        )
        assert result.ok
        assert result.new_contents == {"a.py": "x\nkeep\nX\n"}

    def test_a_later_interval_survives_an_earlier_edit_changing_the_length(self):
        """Scopes are tracked as offsets, so a growing edit does not shift them."""
        content = "a\nb\nc\nd\n"
        edits = [
            edit("a.py", "a", "a1\na2\na3", 0),
            edit("a.py", "d", "D", 1),
        ]
        result = apply_edits(edits, {"a.py": content}, intervals={"a.py": [(1, 1), (4, 4)]})
        assert result.ok, [outcome.reason for outcome in result.outcomes]
        assert result.new_contents == {"a.py": "a1\na2\na3\nb\nc\nD\n"}

    def test_empty_intervals_search_the_whole_file(self):
        """Regression: Agentless raised NameError on this path (postprocess_data.py:747).

        ``file_loc_intervals == []`` referenced ``original`` and ``replace``
        before assignment, so a patch with no localization intervals crashed
        instead of applying. Empty means unscoped here.
        """
        result = apply_edits(
            [edit("a.py", "target", "TARGET")],
            {"a.py": LONG_SOURCE},
            intervals={"a.py": []},
        )
        assert result.ok
        assert result.new_contents == {"a.py": "one\ntwo\nthree\nfour\nTARGET\nsix\n"}

    def test_a_path_absent_from_the_intervals_map_is_unscoped(self):
        result = apply_edits(
            [edit("a.py", "target", "TARGET")],
            {"a.py": LONG_SOURCE},
            intervals={"other.py": [(1, 1)]},
        )
        assert result.ok

    def test_an_out_of_range_interval_is_dropped_not_clamped_open(self):
        result = apply_edits(
            [edit("a.py", "target", "TARGET")],
            {"a.py": LONG_SOURCE},
            intervals={"a.py": [(90, 99)]},
        )
        assert result.outcomes[0].status is EditStatus.OUTSIDE_INTERVALS


ELIDED_SOURCE = "import os\n\n\ndef helper():\n    return 1\n"


class TestElisions:
    def test_a_bare_elision_anchors_to_a_unique_unindented_line(self):
        (parsed,) = parse_blocks(ELIDED).edits
        result = apply_edits([parsed], {"src/app.py": ELIDED_SOURCE})
        assert result.ok
        assert result.new_contents["src/app.py"].startswith("import sys\n\nimport os\n")

    def test_a_leading_elision_on_the_search_side_is_stripped(self):
        result = apply_edits(
            [edit("a.py", "...\ndef helper():", "def helper(flag):")],
            {"a.py": ELIDED_SOURCE},
        )
        assert result.ok
        assert "def helper(flag):" in result.new_contents["a.py"]

    def test_a_leading_elision_on_the_replace_side_is_stripped(self):
        result = apply_edits(
            [edit("a.py", "def helper():", "...\ndef helper(flag):")],
            {"a.py": ELIDED_SOURCE},
        )
        assert result.ok
        assert "...\n" not in result.new_contents["a.py"]

    def test_a_bare_elision_with_an_indented_replacement_is_refused(self):
        result = apply_edits([edit("a.py", "...", "    indented")], {"a.py": ELIDED_SOURCE})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.NO_ANCHOR
        assert "column 1" in outcome.reason

    def test_a_bare_elision_with_no_unique_anchor_is_refused(self):
        result = apply_edits([edit("a.py", "...", "new line")], {"a.py": "dup\ndup\n"})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.NO_ANCHOR
        assert "anchor" in outcome.reason
