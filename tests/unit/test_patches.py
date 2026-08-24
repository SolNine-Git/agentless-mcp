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

ELIDED_BOTH_SIDES = """\
### src/app.py
<<<<<<< SEARCH
...
=======
...
>>>>>>> REPLACE
"""

ELIDED_REPLACEMENT_ONLY = """\
### src/app.py
<<<<<<< SEARCH
...
=======
...

>>>>>>> REPLACE
"""

PROSE_HEADER = """\
I will now fix the rounding in src/app.py:
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
"""

PROSE_UNDER_A_REAL_HEADER = """\
### src/app.py
Here is the fix.
<<<<<<< SEARCH
    return total
=======
    return round(total, 2)
>>>>>>> REPLACE
"""

BEGIN_PATCH_DIALECT = """\
*** Begin Patch
*** Update File: src/app.py
@@ def total():
-    return total
+    return round(total, 2)
*** End Patch
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
            (
                "prose_under_a_real_header",
                PROSE_UNDER_A_REAL_HEADER,
                [("src/app.py", "    return total")],
            ),
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
            ("elided_both_sides", ELIDED_BOTH_SIDES, "both sides of this block are"),
            ("elided_replacement_only", ELIDED_REPLACEMENT_ONLY, "both sides of this block are"),
            ("prose_header", PROSE_HEADER, "names no file"),
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

    @pytest.mark.parametrize(
        ("name", "text"),
        [
            ("empty", ""),
            ("prose", "Here is my fix; apply it carefully.\n"),
            ("begin_patch", BEGIN_PATCH_DIALECT),
        ],
    )
    def test_text_with_no_blocks_at_all_is_an_error_not_zero_edits(self, name, text):
        result = parse_blocks(text)
        assert result.edits == (), name
        assert len(result.errors) == 1, name
        assert not result.ok, name
        assert "no SEARCH/REPLACE blocks found" in result.errors[0].reason, name

    def test_begin_patch_text_is_named_and_redirected(self):
        (error,) = parse_blocks(BEGIN_PATCH_DIALECT).errors
        assert "*** Begin Patch" in error.reason
        assert "rewrite each hunk as a SEARCH/REPLACE block" in error.reason


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


class TestTheRemovedIntervalScoping:
    """The Agentless ``context_segment`` scoping is gone, not merely unused.

    No surface ever supplied it: the CLI patch commands took no line ranges,
    the MCP server exposes no patch tool, and ``patchlint`` matched whole
    files. It left a serialised ``outside_intervals`` status no caller could
    ever receive, and the ``...`` anchor stayed file-wide either way.
    """

    def test_apply_edits_takes_no_scoping_keyword(self):
        with pytest.raises(TypeError):
            apply_edits(
                [edit("a.py", "target", "TARGET")],
                {"a.py": LONG_SOURCE},
                intervals={"a.py": [(4, 6)]},
            )

    def test_the_status_a_scope_used_to_produce_is_gone(self):
        assert not hasattr(EditStatus, "OUTSIDE_INTERVALS")
        assert "outside_intervals" not in {status.value for status in EditStatus}

    def test_a_repeated_line_is_refused_rather_than_narrowed_by_a_range(self):
        """What a scope used to resolve, an ambiguity refusal now reports."""
        result = apply_edits([edit("a.py", "x", "X")], {"a.py": "x\nkeep\nx\n"})
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.AMBIGUOUS
        assert outcome.matches == 2
        assert result.new_contents == {}


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


class TestAnEmptySearchCannotWrite:
    """The guard lived in the two modules that cannot write, not in this one.

    ``core/unidiff`` refuses an empty pre-image against an existing file and
    ``core/patchlint`` refuses it too. Both read; neither writes.
    ``apply_edits`` is the function that produces the new file contents, and
    it accepted the same edit: the empty needle pads to ``"\\n\\n"``, matches
    the first blank line -- or the file's end when there is none -- and the
    replacement is written with the outcome reported as ``applied``.
    """

    def test_an_empty_search_against_an_existing_file_is_refused(self):
        result = apply_edits(
            [Edit(index=0, path="a.py", search="", replace="INJECTED\n")],
            {"a.py": "line one\nline two\n"},
        )

        assert not result.ok
        assert result.outcomes[0].status is EditStatus.NO_ANCHOR
        assert "anchors nowhere" in result.outcomes[0].reason
        assert result.new_contents == {}

    def test_it_is_refused_before_a_blank_line_can_anchor_it(self):
        # The file with a blank line in it is the worse case: the padded
        # needle matches there, so the text lands in the middle rather than
        # at the end, which reads as a deliberate hunk.
        result = apply_edits(
            [Edit(index=0, path="a.py", search="", replace="INJECTED\n")],
            {"a.py": "line one\n\nline two\n"},
        )

        assert not result.ok
        assert result.new_contents == {}

    def test_a_sibling_edit_in_the_same_patch_is_cancelled_with_it(self):
        # The write is all or nothing, so one refused block cancels the patch
        # rather than leaving the other half on disk.
        result = apply_edits(
            [
                Edit(index=0, path="a.py", search="line one", replace="LINE ONE"),
                Edit(index=1, path="a.py", search="", replace="INJECTED\n"),
            ],
            {"a.py": "line one\nline two\n"},
        )

        assert not result.ok
        assert result.outcomes[0].status is EditStatus.APPLIED
        assert result.outcomes[1].status is EditStatus.NO_ANCHOR


class TestAOneWordPathHeader:
    """Path inheritance makes a wrong header worse than a missing one.

    Every single word counted as a path, so a one-word line of prose became
    the filename and the block under it was attributed to that name instead
    of inheriting the correct one. Skipping the prose line is what the
    docstring always claimed and now what the parser does.
    """

    ONE_WORD_PROSE = (
        "### src/a.py\n"
        "<<<<<<< SEARCH\nalpha\n=======\nALPHA\n>>>>>>> REPLACE\n"
        "Done!\n"
        "<<<<<<< SEARCH\nbravo\n=======\nBRAVO\n>>>>>>> REPLACE\n"
    )

    def test_prose_between_blocks_does_not_capture_the_inherited_path(self):
        result = parse_blocks(self.ONE_WORD_PROSE)
        assert result.ok
        assert [block.path for block in result.edits] == ["src/a.py", "src/a.py"]

    @pytest.mark.parametrize("word", ["Done!", "Next:", "Also,", "Fixed."])
    def test_a_word_ending_in_sentence_punctuation_is_prose(self, word):
        text = f"{word}\n<<<<<<< SEARCH\nalpha\n=======\nALPHA\n>>>>>>> REPLACE\n"
        (error,) = parse_blocks(text).errors
        assert error.path is None
        assert "names no file" in error.reason

    @pytest.mark.parametrize(
        "word", ["Makefile", ".gitignore", "src/app.py", "../pkg/mod.rs", "/etc/passwd"]
    )
    def test_a_word_spelled_like_a_filename_is_still_a_path(self, word):
        text = f"{word}\n<<<<<<< SEARCH\nalpha\n=======\nALPHA\n>>>>>>> REPLACE\n"
        (parsed,) = parse_blocks(text).edits
        assert parsed.path == word


class TestANonAsciiPathHeader:
    """A filename is not an ASCII string, and reading it as prose wrote elsewhere.

    The shape test enumerated the characters a path may contain, in ASCII, so
    every accented or non-Latin filename failed it, was skipped as prose, and
    the block under it silently inherited the *previous* block's path. Parse
    reported two edits and no errors, both naming the wrong file.
    """

    TWO_FILES = (
        "src/app.py\n"
        "<<<<<<< SEARCH\nold one\n=======\nnew one\n>>>>>>> REPLACE\n"
        "src/naïve.py\n"
        "<<<<<<< SEARCH\nold two\n=======\nnew two\n>>>>>>> REPLACE\n"
    )

    def test_a_non_ascii_header_is_not_absorbed_by_the_block_above_it(self):
        result = parse_blocks(self.TWO_FILES)
        assert result.errors == ()
        assert [block.path for block in result.edits] == ["src/app.py", "src/naïve.py"]

    @pytest.mark.parametrize(
        "word", ["src/naïve.py", "src/café/app.py", "日本語.py", "über.rs", "naïve.py"]
    )
    def test_a_non_ascii_word_is_a_path(self, word):
        text = f"{word}\n<<<<<<< SEARCH\nalpha\n=======\nALPHA\n>>>>>>> REPLACE\n"
        (parsed,) = parse_blocks(text).edits
        assert parsed.path == word


class TestAnUnreadableHeaderIsRefused:
    """Inheritance is right for a header holding nothing, wrong for one it cannot read.

    Both used to answer None, so a header line the shape test rejected made
    the block below it take the path above it. That is what turned one
    over-narrow character class into a wrong file. A rejected line now has to
    carry evidence of prose -- the punctuation a sentence ends on -- to be
    skipped; anything else is reported against the block it would have
    mis-attributed.
    """

    @staticmethod
    def _two_blocks(header: str) -> str:
        return (
            "### src/a.py\n"
            "<<<<<<< SEARCH\nalpha\n=======\nALPHA\n>>>>>>> REPLACE\n"
            f"{header}\n"
            "<<<<<<< SEARCH\nbravo\n=======\nBRAVO\n>>>>>>> REPLACE\n"
        )

    @pytest.mark.parametrize(
        "header",
        ['"src/b.py', "'src/b.py", "(see src/b.py", "[src/b.py", "{src/b.py"],
    )
    def test_a_header_opening_on_a_delimiter_is_reported(self, header):
        # The one shape that is neither a path nor evidence of prose. A
        # filename opens with `.`, `/`, `~` or `-`, never with a quote or a
        # bracket, and "(see src/b.py" carries an extension on its last
        # component, so no prose test below reaches it.
        result = parse_blocks(self._two_blocks(header))
        assert [block.path for block in result.edits] == ["src/a.py"]
        (error,) = result.errors
        assert error.index == 1
        assert "neither a path nor a sentence" in error.reason
        assert repr(header) in error.reason

    @pytest.mark.parametrize(
        "header",
        [
            # Punctuated like prose.
            "Done!",
            "Next:",
            "Also,",
            "Fixed.",
            "I will now fix the rounding in src/app.py:",
            # More words than a filename carries, punctuation or not. A model
            # narrating between its blocks writes these, so refusing them
            # would lose the header above far more often than the bug this
            # class exists for ever mis-attributed it.
            "Here is the change",
            "I will now fix the rounding in src/app.py",
            "Next I update the parser",
            # Few enough words, but a last component with no extension.
            "Now fixing",
            "and then",
        ],
    )
    def test_a_header_carrying_evidence_of_prose_is_skipped(self, header):
        result = parse_blocks(self._two_blocks(header))
        assert result.ok
        assert [block.path for block in result.edits] == ["src/a.py", "src/a.py"]

    @pytest.mark.parametrize(
        "header",
        [
            "src/b.py",
            "src/naïve.py",
            "src/café/app.py",
            "日本語.py",
            "über.rs",
            "Makefile",
            ".gitignore",
            "my dir/my file.py",
        ],
    )
    def test_a_header_spelled_like_a_path_names_its_own_file(self, header):
        result = parse_blocks(self._two_blocks(header))
        assert result.ok
        assert [block.path for block in result.edits] == ["src/a.py", header]

    def test_a_header_holding_nothing_still_inherits(self):
        result = parse_blocks(self._two_blocks("```"))
        assert result.ok
        assert [block.path for block in result.edits] == ["src/a.py", "src/a.py"]


class TestCrlfIsDiagnosed:
    """A CRLF patch against an LF checkout is a line-ending problem.

    Every content line carried a trailing carriage return into the needle, so
    every block reported "search text not found" -- which reads as a wrong
    search string. ``core.unidiff`` already refuses a structural carriage
    return and says why; this parser now gives the same cause one diagnosis.
    """

    CRLF = "### a.py\r\n<<<<<<< SEARCH\r\nreturn 1\r\n=======\r\nreturn 2\r\n>>>>>>> REPLACE\r\n"

    def test_a_crlf_block_is_refused_by_line_endings_not_by_a_failed_search(self):
        result = parse_blocks(self.CRLF)
        assert result.edits == ()
        (error,) = result.errors
        assert error.path == "a.py"
        assert "CRLF line endings" in error.reason

    def test_a_carriage_return_inside_content_is_kept(self):
        text = "### a.py\n<<<<<<< SEARCH\nreturn 1\r\n=======\nreturn 2\n>>>>>>> REPLACE\n"
        (parsed,) = parse_blocks(text).edits
        assert parsed.search == "return 1\r"


class TestABareElisionSaysWhereItLanded:
    """`...` alone expresses no location, so the outcome has to name one.

    The anchor is chosen from the file rather than from anything the author
    wrote, and reporting only `applied` made the caller accept a placement it
    could not see.
    """

    DOC_FIRST = '"""Doc."""\n\nimport os\n\n\ndef main():\n    return os.getcwd()\n'

    def test_the_anchor_line_is_reported_on_the_applied_outcome(self):
        result = apply_edits(
            [edit("a.py", "...", "def helper():\n    return 2")],
            {"a.py": self.DOC_FIRST},
        )
        (outcome,) = result.outcomes
        assert outcome.status is EditStatus.APPLIED
        assert outcome.reason == 'inserted above line 1: \'"""Doc."""\''

    def test_an_ordinary_edit_still_reports_no_reason(self):
        result = apply_edits([edit("a.py", "import os", "import sys")], {"a.py": self.DOC_FIRST})
        assert result.outcomes[0].reason == ""
