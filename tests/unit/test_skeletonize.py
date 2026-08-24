"""Skeleton defects that the ASCII fixture corpus could not reach.

The byte-exact goldens in :mod:`tests.characterization.test_skeleton` pin what
the skeletonizer does to the committed fixture repositories. Those fixtures
are pure ASCII, brace-on-the-signature-line and comment-to-end-of-line, so
they say nothing about the three index spaces and two comment grammars this
module mixes. Each class below is one property the goldens cannot state.
"""

import pytest

from agentless_mcp.core.skeleton import skeletonize


class TestColumnsAreCharactersNotBytes:
    """Tree-sitter counts a column in UTF-8 bytes; a ``str`` is indexed in
    characters.

    The two agree only while the line is ASCII, and slicing a ``str`` past its
    end never raises, so one accented character earlier on the line moved
    every later cut and the corruption was silent. The last case is the worst
    of the three: source text leaking past the sentinel is the one invariant
    the plan exists to hold.
    """

    def test_a_trailing_comment_after_a_wide_literal_is_still_stripped(self):
        source = 'NAME = "café ☕"  # note\n'
        assert skeletonize(source, "python") == 'NAME = "café ☕"\n'

    def test_no_source_leaks_past_the_sentinel(self):
        source = 'def f(x = "café"): return 1\n'
        assert skeletonize(source, "python") == 'def f(x = "café"): ...\n'

    def test_a_brace_language_comment_is_not_cut_mid_marker(self):
        source = 'const s = "café"; // note\n'
        assert skeletonize(source, "javascript") == 'const s = "café";\n'


class TestCodeAfterAComment:
    """A ``/* ... */`` comment can sit between two pieces of an expression.

    Keeping only the text before it rewrote a constant into something that
    still reads as a complete constant with a different value, which is worse
    than dropping the line. The module docstring promises module-level
    assignments survive so constants are readable.
    """

    def test_an_inline_block_comment_keeps_the_code_after_it(self):
        source = "package p\n\nconst X = 1 /* note */ + 2\n"
        assert skeletonize(source, "go") == "package p\n\nconst X = 1 + 2\n"

    def test_a_leading_block_comment_keeps_the_statement(self):
        source = "const x = /* wide */ 5;\n"
        assert skeletonize(source, "javascript") == "const x = 5;\n"

    def test_a_multi_line_block_comment_keeps_the_tail_of_its_last_line(self):
        source = "let z = /* one\n two */ 9;\n"
        assert skeletonize(source, "javascript") == "let z = 9;\n"

    def test_a_comment_owning_its_whole_line_is_still_dropped(self):
        source = "// note\nconst x = 5;\n"
        assert skeletonize(source, "javascript") == "const x = 5;\n"

    def test_a_trailing_comment_is_still_trimmed_off_its_line(self):
        source = "x = 1  # note\n"
        assert skeletonize(source, "python") == "x = 1\n"


class TestAnAllmanBodyKeepsItsBraces:
    """A body whose block token opens its own line.

    The local read as the signature's line and was in fact the body's first
    line, so the branch that keeps a brace never fired for Allman style: the
    braces went with the body and left a bare sentinel where a block belongs.
    The module docstring says the output still reads as code.
    """

    @pytest.mark.parametrize("language", ["c", "cpp"])
    def test_an_allman_function_keeps_a_braced_body(self, language):
        source = "int f(int a)\n{\n    return a;\n}\n"
        assert skeletonize(source, language) == "int f(int a)\n{ ... }\n"

    def test_an_allman_method_keeps_its_own_indentation(self):
        source = "class A\n{\n    int f()\n    {\n        return 1;\n    }\n}\n"
        assert skeletonize(source, "java") == "class A\n{\n    int f()\n    { ... }\n}\n"

    def test_a_brace_on_the_signature_line_is_unchanged(self):
        source = "int f(int a) {\n    return a;\n}\n"
        assert skeletonize(source, "c") == "int f(int a) { ... }\n"

    def test_an_indented_body_still_gets_a_bare_sentinel(self):
        source = "def f(a):\n    return a\n"
        assert skeletonize(source, "python") == "def f(a):\n    ...\n"


class TestTheTrailingBlankTrim:
    """Blankness is decided on the source text, not on the rendered line.

    The trim re-parsed its own output with ``strip().rstrip("|")``, which is
    truthy the moment a ``N| `` prefix carries a digit, so it never fired
    under ``number_lines`` and a numbered skeleton kept a trailing blank line
    the unnumbered one dropped.
    """

    SOURCE = "x = 1\n\n\n# c\n\n"

    def test_the_unnumbered_render_ends_on_content(self):
        assert skeletonize(self.SOURCE, "python") == "x = 1\n"

    def test_the_numbered_render_ends_on_content_too(self):
        assert skeletonize(self.SOURCE, "python", number_lines=True) == "1| x = 1\n"

    def test_an_interior_blank_run_still_collapses_to_one_line(self):
        source = "x = 1\n\n\ny = 2\n"
        assert skeletonize(source, "python", number_lines=True) == "1| x = 1\n2| \n4| y = 2\n"


class TestAPrefixedDocstring:
    """A raw docstring starts with ``r``, not with its quote run.

    Matching the run against the whole literal failed, so the literal was
    wrapped in a second pair of quotes and the output did not parse. The
    prefix is kept, because dropping it changes what the literal means.
    """

    def test_a_raw_docstring_renders_as_one_parseable_line(self):
        source = 'def f():\n    r"""Raw docs."""\n    return 1\n'
        assert skeletonize(source, "python", docstrings=True) == (
            'def f():\n    r"""Raw docs."""\n    ...\n'
        )

    def test_a_plain_docstring_is_unchanged(self):
        source = 'def f():\n    """Docs."""\n    return 1\n'
        assert skeletonize(source, "python", docstrings=True) == (
            'def f():\n    """Docs."""\n    ...\n'
        )

    def test_a_module_docstring_with_a_prefix_keeps_it(self):
        source = 'r"""Module \\d docs."""\n\nX = 1\n'
        assert skeletonize(source, "python", docstrings=True) == (
            'r"""Module \\d docs."""\n\nX = 1\n'
        )
