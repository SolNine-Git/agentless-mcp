"""The unified-diff reader: every construct is mapped, noted, or refused.

The rule this module exists to hold: nothing a diff contains may be dropped
silently. A construct either becomes edits, or becomes a note the report
renders, or becomes a refusal that names the file and says why. Each test below
pins one construct to one of those three, and the refusals are asserted on their
*reasons* rather than merely on being refused, because a refusal a reader cannot
act on is only a slower silence.
"""

from agentless_mcp.core import unidiff

SIMPLE = """\
diff --git a/app.py b/app.py
index 5cd584d..4da2345 100644
--- a/app.py
+++ b/app.py
@@ -1,2 +1,2 @@
 import json
-value = 1
+value = 2
"""


def only_error(text):
    """Parse ``text`` and return the single refusal it produced."""
    parsed = unidiff.parse_unified_diff(text)
    assert parsed.result.edits == ()
    assert len(parsed.result.errors) == 1
    return parsed.result.errors[0]


def edits_of(text):
    """Parse ``text``, asserting it refused nothing, and return its edits."""
    parsed = unidiff.parse_unified_diff(text)
    assert parsed.result.errors == (), parsed.result.errors
    return parsed.result.edits


class TestTheOrdinaryCase:
    def test_a_hunk_becomes_one_edit_with_context_on_both_sides(self):
        (edit,) = edits_of(SIMPLE)
        assert edit.path == "app.py"
        assert edit.search == "import json\nvalue = 1"
        assert edit.replace == "import json\nvalue = 2"
        assert edit.index == 0

    def test_the_a_and_b_prefixes_git_adds_are_stripped(self):
        (edit,) = edits_of(SIMPLE)
        assert edit.path == "app.py"

    def test_a_diff_with_no_prefixes_keeps_its_paths(self):
        text = """\
--- app.py
+++ app.py
@@ -1,1 +1,1 @@
-old
+new
"""
        (edit,) = edits_of(text)
        assert edit.path == "app.py"

    def test_a_plain_diff_u_drops_its_timestamp_column(self):
        text = "--- app.py\t2020-01-01 00:00:00.000000000 +0000\n"
        text += "+++ app.py\t2020-01-02 00:00:00.000000000 +0000\n"
        text += "@@ -1,1 +1,1 @@\n-old\n+new\n"
        (edit,) = edits_of(text)
        assert edit.path == "app.py"

    def test_index_lines_carry_no_content_and_are_consumed(self):
        assert len(edits_of(SIMPLE)) == 1

    def test_omitted_counts_mean_one_line_a_side(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -3 +3 @@\n-old\n+new\n"
        (edit,) = edits_of(text)
        assert edit.search == "old"
        assert edit.replace == "new"

    def test_a_hunk_header_may_carry_the_enclosing_symbol(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -3,1 +3,1 @@ def render(payload):\n-old\n+new\n"
        (edit,) = edits_of(text)
        assert edit.search == "old"


class TestSeveralHunksAndFiles:
    def test_each_hunk_of_a_file_becomes_its_own_edit_in_order(self):
        text = """\
--- a/app.py
+++ b/app.py
@@ -1,1 +1,1 @@
-first
+FIRST
@@ -9,1 +9,1 @@
-second
+SECOND
"""
        first, second = edits_of(text)
        assert (first.index, first.search) == (0, "first")
        assert (second.index, second.search) == (1, "second")
        assert first.path == second.path == "app.py"

    def test_hunk_ordinals_run_across_the_whole_diff(self):
        text = (
            SIMPLE
            + """\
diff --git a/other.py b/other.py
--- a/other.py
+++ b/other.py
@@ -1,1 +1,1 @@
-old
+new
"""
        )
        first, second = edits_of(text)
        assert (first.path, first.index) == ("app.py", 0)
        assert (second.path, second.index) == ("other.py", 1)

    def test_a_context_line_that_looks_like_a_header_is_content(self):
        # The removed line is `-- still content`, whose diff spelling begins
        # `--- `. Consuming the body by count is what keeps this from reading as
        # the next file's header.
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,2 +1,1 @@\n--- still content\n-gone\n+new\n"
        (edit,) = edits_of(text)
        assert edit.search == "-- still content\ngone"
        assert edit.replace == "new"


class TestFilesThatAppearOrDisappear:
    def test_a_new_file_becomes_an_edit_with_an_empty_pre_image(self):
        text = """\
diff --git a/new.py b/new.py
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/new.py
@@ -0,0 +1,2 @@
+import json
+value = 1
"""
        (edit,) = edits_of(text)
        assert edit.path == "new.py"
        assert edit.search == ""
        assert edit.replace == "import json\nvalue = 1"

    def test_a_deleted_file_becomes_an_edit_with_an_empty_post_image(self):
        text = """\
diff --git a/gone.py b/gone.py
deleted file mode 100644
index 1234567..0000000
--- a/gone.py
+++ /dev/null
@@ -1,2 +0,0 @@
-import json
-value = 1
"""
        (edit,) = edits_of(text)
        assert edit.path == "gone.py"
        assert edit.search == "import json\nvalue = 1"
        assert edit.replace == ""

    def test_both_sides_dev_null_names_no_file_at_all(self):
        text = "--- /dev/null\n+++ /dev/null\n@@ -0,0 +0,0 @@\n"
        assert "names no file at all" in only_error(text).reason


class TestConstructsWithNothingToCheck:
    def test_a_mode_only_change_is_a_note_rather_than_a_dropped_section(self):
        text = """\
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
"""
        parsed = unidiff.parse_unified_diff(text)
        assert parsed.result.edits == ()
        assert parsed.result.errors == ()
        (note,) = parsed.notes
        assert note.path == "run.sh"
        assert "mode change only" in note.reason

    def test_a_git_binary_payload_is_a_note(self):
        text = """\
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000..1234567
GIT binary patch
literal 8
Pc$~9WzzzzzzzzzzzzzZ

"""
        parsed = unidiff.parse_unified_diff(text)
        assert parsed.result.errors == ()
        (note,) = parsed.notes
        assert note.path == "logo.png"
        assert "binary" in note.reason

    def test_a_textual_binary_marker_is_a_note(self):
        text = """\
diff --git a/logo.png b/logo.png
index 1234567..89abcde 100644
Binary files a/logo.png and b/logo.png differ
"""
        parsed = unidiff.parse_unified_diff(text)
        assert parsed.result.errors == ()
        assert parsed.notes[0].path == "logo.png"

    def test_a_binary_file_does_not_stop_the_rest_of_the_diff(self):
        text = (
            """\
diff --git a/logo.png b/logo.png
index 1234567..89abcde 100644
Binary files a/logo.png and b/logo.png differ
"""
            + SIMPLE
        )
        parsed = unidiff.parse_unified_diff(text)
        assert parsed.result.errors == ()
        assert len(parsed.notes) == 1
        assert [edit.path for edit in parsed.result.edits] == ["app.py"]


class TestRenamesAndCopies:
    def test_rename_headers_are_refused_because_an_edit_carries_one_path(self):
        text = """\
diff --git a/old.py b/new.py
similarity index 90%
rename from old.py
rename to new.py
"""
        assert "renames or copies" in only_error(text).reason

    def test_a_copy_is_refused_the_same_way(self):
        text = """\
diff --git a/old.py b/new.py
similarity index 100%
copy from old.py
copy to new.py
"""
        assert "renames or copies" in only_error(text).reason

    def test_two_different_paths_are_refused_even_without_a_rename_header(self):
        # The guard keys on the invariant -- the two sides name different files
        # -- not on the header spelling, which git omits in some diff options.
        text = "--- a/old.py\n+++ b/new.py\n@@ -1,1 +1,1 @@\n-old\n+new\n"
        reason = only_error(text).reason
        assert "renames old.py to new.py" in reason


class TestPathsThisReaderWillNotGuessAt:
    def test_a_c_quoted_path_is_refused_rather_than_decoded(self):
        text = '--- "a/na\\303\\257ve.py"\n+++ "b/na\\303\\257ve.py"\n@@ -1,1 +1,1 @@\n-old\n+new\n'
        reason = only_error(text).reason
        assert "C-quoted path" in reason
        assert "wrong file" in reason

    def test_only_one_of_the_two_headers_is_refused(self):
        text = "diff --git a/app.py b/app.py\n--- a/app.py\n@@ -1,1 +1,1 @@\n-old\n+new\n"
        assert "only one of the '---' and '+++' headers" in only_error(text).reason


class TestLineEndings:
    def test_a_carriage_return_inside_content_is_kept_because_the_file_has_it(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,1 +1,1 @@\n-old\r\n+new\r\n"
        (edit,) = edits_of(text)
        assert edit.search == "old\r"
        assert edit.replace == "new\r"

    def test_a_carriage_return_on_a_structural_line_is_refused(self):
        text = "--- a/app.py\r\n+++ b/app.py\r\n@@ -1,1 +1,1 @@\r\n-old\r\n+new\r\n"
        reason = only_error(text).reason
        assert "CRLF" in reason
        assert "convert it to LF" in reason

    def test_a_carriage_return_on_a_hunk_header_is_refused(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,1 +1,1 @@\r\n-old\n+new\n"
        assert "CRLF" in only_error(text).reason

    def test_the_no_newline_marker_is_consumed_rather_than_read_as_content(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,1 +1,1 @@\n-old\n\\ No newline at end of file\n"
        text += "+new\n\\ No newline at end of file\n"
        (edit,) = edits_of(text)
        assert edit.search == "old"
        assert edit.replace == "new"


class TestHunkBodies:
    def test_a_body_shorter_than_its_header_claims_is_refused(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,5 +1,5 @@\n context\n-old\n+new\n"
        reason = only_error(text).reason
        assert "truncated diff?" in reason
        assert "app.py" in reason

    def test_a_body_longer_than_its_header_claims_is_refused(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,1 +1,1 @@\n-one\n-two\n+new\n"
        assert "does not match" in only_error(text).reason

    def test_a_body_line_with_no_marker_is_refused(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,2 +1,1 @@\n context\nnaked line\n+new\n"
        assert "is not ' ', '-', '+'" in only_error(text).reason

    def test_a_bare_empty_line_is_read_as_an_empty_context_line(self):
        # Editors and mail clients strip the trailing space off a context line
        # for a blank line; every patch reader tolerates the result.
        text = "--- a/app.py\n+++ b/app.py\n@@ -1,3 +1,3 @@\n one\n\n-old\n+new\n"
        (edit,) = edits_of(text)
        assert edit.search == "one\n\nold"
        assert edit.replace == "one\n\nnew"

    def test_a_hunk_header_that_is_not_a_hunk_header_is_refused(self):
        text = "--- a/app.py\n+++ b/app.py\n@@ this is not a hunk header @@\n-old\n+new\n"
        assert "is not '@@ -old,count +new,count @@'" in only_error(text).reason

    def test_a_file_header_with_no_hunks_is_refused(self):
        text = "--- a/app.py\n+++ b/app.py\ndiff --git a/other.py b/other.py\n"
        assert "no hunks" in only_error(text).reason


class TestZeroContext:
    def test_a_zero_context_hunk_into_an_existing_file_is_refused(self):
        # An empty pre-image anchors nowhere, and apply_edits pads it into
        # "\n\n", which matches every blank line in the file.
        text = "--- a/app.py\n+++ b/app.py\n@@ -10,0 +11,1 @@\n+added\n"
        reason = only_error(text).reason
        assert "zero-context hunk" in reason
        assert "drop -U0" in reason

    def test_a_new_file_may_have_an_empty_pre_image(self):
        text = "--- /dev/null\n+++ b/new.py\n@@ -0,0 +1,1 @@\n+added\n"
        (edit,) = edits_of(text)
        assert edit.search == ""


class TestWholeDocuments:
    def test_a_combined_merge_diff_is_refused(self):
        text = """\
diff --cc app.py
index 1234567,89abcde..0000000
--- a/app.py
+++ b/app.py
@@@ -1,1 -1,1 +1,1 @@@
- one
 -two
++three
"""
        assert "combined (merge) diff" in only_error(text).reason

    def test_text_that_is_not_a_diff_at_all_is_refused(self):
        assert "does not look like a unified diff" in only_error("just some prose\n").reason

    def test_an_empty_document_is_refused(self):
        assert "does not look like a unified diff" in only_error("").reason

    def test_an_unrecognised_extended_header_is_refused_rather_than_skipped(self):
        text = "diff --git a/app.py b/app.py\nquantum entanglement index 7\n--- a/app.py\n"
        text += "+++ b/app.py\n@@ -1,1 +1,1 @@\n-old\n+new\n"
        assert "unrecognised header line" in only_error(text).reason

    def test_a_refused_section_is_not_then_read_a_second_time(self):
        # A `diff --git` section runs to the next `diff --git`, so a refusal
        # raised above its `---` line must not resume on that line and parse the
        # very section it just refused.
        text = "diff --git a/app.py b/app.py\nquantum entanglement index 7\n--- a/app.py\n"
        text += "+++ b/app.py\n@@ -1,1 +1,1 @@\n-old\n+new\n"
        parsed = unidiff.parse_unified_diff(text)
        assert parsed.result.edits == ()

    def test_a_refusal_in_one_file_does_not_swallow_the_next(self):
        text = "diff --git a/bad.py b/bad.py\nquantum entanglement index 7\n" + SIMPLE
        parsed = unidiff.parse_unified_diff(text)
        assert len(parsed.result.errors) == 1
        assert [edit.path for edit in parsed.result.edits] == ["app.py"]

    def test_a_format_patch_prologue_is_passed_over(self):
        text = (
            """\
From 1234567890abcdef Mon Sep 17 00:00:00 2001
From: Someone <someone@example.invalid>
Subject: [PATCH] make a change

A sentence about the change.
---
 app.py | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

"""
            + SIMPLE
            + """\
--
2.39.0
"""
        )
        (edit,) = edits_of(text)
        assert edit.path == "app.py"


class TestOrientation:
    """The tree the checks run against has to be the diff's base."""

    def test_a_pre_image_that_is_in_the_tree_passes(self):
        (edit,) = edits_of(SIMPLE)
        assert unidiff.orientation([edit], {"app.py": "import json\nvalue = 1\n"}) == ()

    def test_a_diff_already_applied_is_reported_with_its_remedy(self):
        (edit,) = edits_of(SIMPLE)
        (problem,) = unidiff.orientation([edit], {"app.py": "import json\nvalue = 2\n"})
        assert problem.path == "app.py"
        assert "already applied to --repo" in problem.reason
        assert "point --repo at a checkout of the diff's base" in problem.reason
        assert "merge-base" in problem.reason

    def test_a_diff_against_a_third_state_is_reported(self):
        (edit,) = edits_of(SIMPLE)
        (problem,) = unidiff.orientation([edit], {"app.py": "something else entirely\n"})
        assert "neither this hunk's pre-image nor its post-image" in problem.reason
        assert "point --repo at a checkout of the diff's base" in problem.reason

    def test_a_new_file_that_already_exists_means_the_tree_is_not_the_base(self):
        text = "--- /dev/null\n+++ b/new.py\n@@ -0,0 +1,1 @@\n+added\n"
        (edit,) = edits_of(text)
        (problem,) = unidiff.orientation([edit], {"new.py": "added\n"})
        assert "already exists in --repo" in problem.reason
        assert "point --repo at a checkout of the diff's base" in problem.reason

    def test_a_new_file_absent_from_the_tree_passes(self):
        text = "--- /dev/null\n+++ b/new.py\n@@ -0,0 +1,1 @@\n+added\n"
        (edit,) = edits_of(text)
        assert unidiff.orientation([edit], {}) == ()

    def test_a_path_with_no_text_is_not_judged(self):
        # `texts` holds only the files the scan parsed, so a diff touching a
        # README must not be refused for being absent from a tree that has it.
        (edit,) = edits_of(SIMPLE)
        assert unidiff.orientation([edit], {}) == ()

    def test_one_file_is_reported_once_however_many_hunks_disagree(self):
        text = """\
--- a/app.py
+++ b/app.py
@@ -1,1 +1,1 @@
-alpha
+ALPHA
@@ -9,1 +9,1 @@
-beta
+BETA
"""
        edits = edits_of(text)
        problems = unidiff.orientation(edits, {"app.py": "ALPHA\nBETA\n"})
        assert len(problems) == 1
        assert problems[0].path == "app.py"

    def test_matching_is_whole_line_so_a_substring_is_not_a_pre_image(self):
        (edit,) = edits_of(SIMPLE)
        problems = unidiff.orientation([edit], {"app.py": "ximport jsonx\nxvalue = 1x\n"})
        assert len(problems) == 1
