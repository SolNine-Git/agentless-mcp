"""Equivalence keys and syntax verdicts, across python, typescript and go.

Two properties are what the vote in Phase 3 will rest on, so both are pinned
in all three languages rather than in Python alone: a change that only moves
comments or whitespace must hash to the same key as no change at all, and a
change that moves code between blocks must not.

The dedent case is the adversarial one the plan flagged as a risk, and it is
Python-specific: `}` and `end` are tokens, a DEDENT is not. The first test in
:class:`TestBlockStructure` fails for python without the explicit block
marker, and passes for typescript and go on the braces alone.

:class:`TestEquivalence`'s semantic case is the one that decided the token
rule. `left + right` versus `left - right` hashed identically while only
*named* leaves were emitted, because tree-sitter spells an operator as an
anonymous node -- so the stream carries every leaf.
"""

from dataclasses import dataclass

import pytest

from agentless_mcp.core.normalize import (
    BLOCK_CLOSE,
    BLOCK_OPEN,
    TEXT_ONLY_KEY_PREFIX,
    equivalence_key,
    file_key,
    normalized_stream,
    syntax_delta,
)

PY_BEFORE = '''\
"""Module."""

TOTAL = 1


def add(left, right):
    # Sum them.
    return left + right
'''

PY_COMMENTED = '''\
"""Module."""

TOTAL = 1


def add(left, right):
    # Sum them, carefully, having thought about it.
    # And a second line of commentary.
    return left + right
'''

PY_RESPACED = '''\
"""Module."""

TOTAL = 1


def add(left, right):
    # Sum them.

    return   left+right
'''

PY_SEMANTIC = '''\
"""Module."""

TOTAL = 1


def add(left, right):
    # Sum them.
    return left - right
'''

TS_BEFORE = """\
// A total.
export const TOTAL = 1;

export function add(left: number, right: number): number {
  return left + right;
}
"""

TS_COMMENTED = """\
/* A running total, kept here. */
export const TOTAL = 1;

export function add(left: number, right: number): number {
  // Sum them.
  return left + right;
}
"""

TS_RESPACED = """\
// A total.
export const TOTAL = 1;

export function add(left: number, right: number): number {
    return left+right;

}
"""

TS_SEMANTIC = """\
// A total.
export const TOTAL = 1;

export function add(left: number, right: number): number {
  return left - right;
}
"""

GO_BEFORE = """\
package app

// Total counts things.
const Total = 1

func Add(left int, right int) int {
\treturn left + right
}
"""

GO_COMMENTED = """\
package app

/* Total counts things, and is documented at length. */
const Total = 1

func Add(left int, right int) int {
\t// Sum them.
\treturn left + right
}
"""

GO_RESPACED = """\
package app

// Total counts things.
const Total = 1

func Add(left int, right int) int {

\treturn left+right
}
"""

GO_SEMANTIC = """\
package app

// Total counts things.
const Total = 1

func Add(left int, right int) int {
\treturn left - right
}
"""


@dataclass(frozen=True)
class Case:
    """One language's before/after corpus, as a single parameter."""

    language: str
    path: str
    before: str
    commented: str
    respaced: str
    semantic: str


LANGUAGE_CASES = [
    Case("python", "app.py", PY_BEFORE, PY_COMMENTED, PY_RESPACED, PY_SEMANTIC),
    Case("typescript", "app.ts", TS_BEFORE, TS_COMMENTED, TS_RESPACED, TS_SEMANTIC),
    Case("go", "app.go", GO_BEFORE, GO_COMMENTED, GO_RESPACED, GO_SEMANTIC),
]


def key_for(path, language, old, new):
    """Build a one-file patch key."""
    return equivalence_key({path: (old, new)}, lambda _: language)


@pytest.mark.parametrize("case", LANGUAGE_CASES, ids=lambda case: case.language)
class TestEquivalence:
    def test_a_comment_only_change_keeps_the_key(self, case):
        assert key_for(case.path, case.language, case.before, case.commented) == key_for(
            case.path, case.language, case.before, case.before
        )

    def test_a_whitespace_only_change_keeps_the_key(self, case):
        assert key_for(case.path, case.language, case.before, case.respaced) == key_for(
            case.path, case.language, case.before, case.before
        )

    def test_a_semantic_change_moves_the_key(self, case):
        assert key_for(case.path, case.language, case.before, case.semantic) != key_for(
            case.path, case.language, case.before, case.before
        )


PY_NESTED = """\
def run(flag):
    if flag:
        first()
        second()
"""

PY_DEDENTED = """\
def run(flag):
    if flag:
        first()
    second()
"""

TS_NESTED = """\
function run(flag: boolean) {
  if (flag) {
    first();
    second();
  }
}
"""

TS_DEDENTED = """\
function run(flag: boolean) {
  if (flag) {
    first();
  }
  second();
}
"""

GO_NESTED = """\
package app

func Run(flag bool) {
\tif flag {
\t\tfirst()
\t\tsecond()
\t}
}
"""

GO_DEDENTED = """\
package app

func Run(flag bool) {
\tif flag {
\t\tfirst()
\t}
\tsecond()
}
"""


class TestSemanticComments:
    """Comment forms a compiler, a type checker or a lint gate reads.

    Dropping every comment node made these hash as no change at all, so a
    candidate that flips a Go build constraint or strips a ``# type:``
    annotation was clustered with the candidate that changes nothing and
    credited its votes.
    """

    @pytest.mark.parametrize(
        ("language", "path", "before", "after"),
        [
            (
                "go",
                "app.go",
                "//go:build linux\n\npackage app\n",
                "//go:build windows\n\npackage app\n",
            ),
            (
                "go",
                "app.go",
                "// +build linux\n\npackage app\n",
                "// +build windows\n\npackage app\n",
            ),
            (
                "python",
                "run.py",
                "#!/usr/bin/env python3\nx = 1\n",
                "#!/usr/bin/env python2\nx = 1\n",
            ),
            ("python", "app.py", "x = f()  # type: int\n", "x = f()  # type: str\n"),
            ("python", "app.py", "import os  # noqa: F401\n", "import os\n"),
            (
                "typescript",
                "app.ts",
                "// @ts-expect-error\nconst x = f();\n",
                "const x = f();\n",
            ),
            (
                "typescript",
                "app.ts",
                "// eslint-disable-next-line no-eval\nconst x = f();\n",
                "const x = f();\n",
            ),
        ],
        ids=["go-build", "go-plus-build", "shebang", "type-comment", "noqa", "ts-expect", "eslint"],
    )
    def test_a_directive_comment_moves_the_key(self, language, path, before, after):
        assert key_for(path, language, before, after) != key_for(path, language, before, before)

    @pytest.mark.parametrize(
        ("language", "before", "after"),
        [
            ("python", "# a pragmatic choice\nx = 1\n", "# a different note\nx = 1\n"),
            (
                "go",
                "package app\n\n// Total counts.\nconst Total = 1\n",
                "package app\n\n// Counts.\nconst Total = 1\n",
            ),
        ],
        ids=["python-prose", "go-prose"],
    )
    def test_prose_still_normalises_away(self, language, before, after):
        path = "app.py" if language == "python" else "app.go"
        assert key_for(path, language, before, after) == key_for(path, language, before, before)

    def test_reflowing_a_directives_whitespace_is_not_a_change(self):
        spaced = "x = f()  #  type:  int\n"
        original = "x = f()  # type: int\n"
        assert key_for("app.py", "python", original, spaced) == key_for(
            "app.py", "python", original, original
        )


class TestBlockStructure:
    """The adversarial case: same tokens, different blocks."""

    @pytest.mark.parametrize(
        ("language", "nested", "dedented"),
        [
            ("python", PY_NESTED, PY_DEDENTED),
            ("typescript", TS_NESTED, TS_DEDENTED),
            ("go", GO_NESTED, GO_DEDENTED),
        ],
    )
    def test_moving_a_statement_out_of_a_block_changes_the_key(self, language, nested, dedented):
        assert file_key(nested, dedented, language) != file_key(nested, nested, language)

    def test_pythons_tokens_alone_would_not_have_caught_it(self):
        """Why the python block marker exists, stated as a test not a comment."""
        nested = [
            token
            for token in normalized_stream(PY_NESTED, "python").split(" ")
            if token not in (BLOCK_OPEN, BLOCK_CLOSE)
        ]
        dedented = [
            token
            for token in normalized_stream(PY_DEDENTED, "python").split(" ")
            if token not in (BLOCK_OPEN, BLOCK_CLOSE)
        ]
        assert nested == dedented

    def test_an_operator_flip_moves_the_key(self):
        """Why every leaf is emitted and not only the named ones."""
        plus = "def add(a, b):\n    return a + b\n"
        minus = "def add(a, b):\n    return a - b\n"
        assert file_key(plus, minus, "python") != file_key(plus, plus, "python")


class TestPatchKey:
    def test_the_same_edit_in_two_files_gives_two_keys(self):
        one = equivalence_key({"a.py": (PY_BEFORE, PY_SEMANTIC)}, lambda _: "python")
        other = equivalence_key({"b.py": (PY_BEFORE, PY_SEMANTIC)}, lambda _: "python")
        assert one != other

    def test_file_order_does_not_matter(self):
        forward = equivalence_key(
            {"a.py": (PY_BEFORE, PY_SEMANTIC), "b.py": (PY_BEFORE, PY_COMMENTED)},
            lambda _: "python",
        )
        backward = equivalence_key(
            {"b.py": (PY_BEFORE, PY_COMMENTED), "a.py": (PY_BEFORE, PY_SEMANTIC)},
            lambda _: "python",
        )
        assert forward == backward

    def test_an_unsupported_language_falls_back_to_whitespace_normalised_text(self):
        respaced = "hello    world\n"
        assert file_key("hello world\n", respaced, None) == file_key(
            "hello world\n", "hello world\n", None
        )
        assert file_key("hello world\n", "hello there\n", None) != file_key(
            "hello world\n", "hello world\n", None
        )


PY_BROKEN = '''\
"""Module."""

TOTAL = 1


def add(left, right:
    return left + right
'''

TS_BROKEN = """\
export function add(left: number, right: number): number {
  return left + right;
"""

GO_BROKEN = """\
package app

func Add(left int, right int) int {
\treturn left +
"""

PY_ALREADY_BROKEN = """\
def broken(:
    pass


def fine():
    return 1
"""

PY_ALREADY_BROKEN_EDITED = """\
def broken(:
    pass


def fine():
    return 2
"""


class TestSyntaxDelta:
    @pytest.mark.parametrize(
        ("language", "clean", "broken"),
        [
            ("python", PY_BEFORE, PY_BROKEN),
            ("typescript", TS_BEFORE, TS_BROKEN),
            ("go", GO_BEFORE, GO_BROKEN),
        ],
    )
    def test_a_broken_replacement_is_caught(self, language, clean, broken):
        verdict = syntax_delta(clean, broken, language)
        assert not verdict.ok
        assert verdict.old_errors == 0
        assert verdict.new_errors > 0
        assert "new parse errors" in verdict.detail

    def test_a_clean_edit_is_ok(self):
        verdict = syntax_delta(PY_BEFORE, PY_SEMANTIC, "python")
        assert verdict.ok
        assert verdict.new_errors == 0

    def test_a_file_that_was_already_broken_stays_ok(self):
        """Baseline delta, not absolute: a pre-existing error is not this patch's."""
        verdict = syntax_delta(PY_ALREADY_BROKEN, PY_ALREADY_BROKEN_EDITED, "python")
        assert verdict.old_errors > 0
        assert verdict.new_errors == verdict.old_errors
        assert verdict.ok
        assert "already had" in verdict.detail

    def test_an_unknown_language_says_so_instead_of_claiming_a_check(self):
        verdict = syntax_delta("anything", "anything else", None)
        assert verdict.ok
        assert "not checked" in verdict.detail

    def test_not_checked_is_its_own_field_not_only_free_text(self):
        """`ok` is a delta over what a parser saw. With no parser it saw
        nothing, and a caller gating on a real parse needs that as a value
        rather than as a substring of `detail`."""
        assert not syntax_delta("anything", "anything else", None).checked
        assert syntax_delta(PY_BEFORE, PY_SEMANTIC, "python").checked


class TestAKeyBuiltWithoutAGrammarSaysSo:
    """The text-only fallback is a weaker claim than an AST key, and the
    value has to carry that.

    Whitespace-collapsed raw text is not a normalisation for a file whose
    meaning depends on whitespace. Two YAML files with different nesting
    collapse to one stream, and the hash that reached `core.vote` and
    patchlint's near-duplicate check was indistinguishable from a real AST
    key, so a false duplicate was a wrong answer the vote acted on.
    """

    def test_a_grammar_backed_key_stays_a_bare_digest(self):
        key = file_key(PY_BEFORE, PY_SEMANTIC, "python")
        assert not key.startswith(TEXT_ONLY_KEY_PREFIX)
        assert len(key) == 64

    def test_a_fallback_key_is_labelled(self):
        key = file_key("a: 1\n", "a: 2\n", None)
        assert key.startswith(TEXT_ONLY_KEY_PREFIX)

    def test_the_yaml_collision_is_still_there_but_now_visible(self):
        nested = file_key("x", "a:\n  b: 1\n", None)
        deeper = file_key("x", "a:\n    b: 1\n", None)
        assert nested == deeper
        assert nested.startswith(TEXT_ONLY_KEY_PREFIX)

    def test_one_ungrammared_file_labels_the_whole_patch_key(self):
        mixed = equivalence_key(
            {"a.py": (PY_BEFORE, PY_SEMANTIC), "b.yml": ("a: 1\n", "a: 2\n")},
            lambda path: "python" if path.endswith(".py") else None,
        )
        assert mixed.startswith(TEXT_ONLY_KEY_PREFIX)

    def test_an_all_grammar_patch_key_stays_bare(self):
        key = equivalence_key({"a.py": (PY_BEFORE, PY_SEMANTIC)}, lambda _: "python")
        assert not key.startswith(TEXT_ONLY_KEY_PREFIX)
        assert len(key) == 64


class TestDirectiveFamilies:
    """A directive is read by something, so rewriting one is a real change.

    The table stopped at the languages that were being measured, and left out
    the families this repository's own stack reads. One test per family, not
    per literal: the point is the family.
    """

    @pytest.mark.parametrize(
        "directive",
        [
            "# ruff: noqa",
            "# ruff: noqa: E501",
            "# pylint: disable=too-many-locals",
            "# mypy: disable-error-code=misc",
            "# flake8: noqa",
            "# fmt: off",
            "# isort: skip_file",
            "# -*- coding: utf-8 -*-",
            "# noqa: E501",
            "# type: ignore[arg-type]",
            "# pragma: no cover",
        ],
    )
    def test_adding_a_directive_moves_the_key(self, directive):
        plain = "TOTAL = 1\n"
        with_directive = f"{directive}\nTOTAL = 1\n"
        assert file_key(plain, with_directive, "python") != file_key(plain, plain, "python")

    def test_prose_is_still_dropped(self):
        plain = "TOTAL = 1\n"
        commented = "# Sum of everything.\nTOTAL = 1\n"
        assert file_key(plain, commented, "python") == file_key(plain, plain, "python")

    def test_a_word_that_merely_starts_like_a_directive_is_prose(self):
        plain = "TOTAL = 1\n"
        prose = "# pragmatic choices were made\nTOTAL = 1\n"
        assert file_key(plain, prose, "python") == file_key(plain, plain, "python")
