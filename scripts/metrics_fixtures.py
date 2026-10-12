"""Write the metrics test fixtures: each input with the units, blocks and SLOC lines it measures to.

Development-time only. Run it from the repository root:

    uv run python scripts/metrics_fixtures.py

The inputs are every file in tests/characterization/fixtures/complexity_*/, the first half of each
of those files, and the snippets below. Each case holds its source and either the units, the
blocks and the SLOC lines, or the refusal. With --check, the script compares its output with the
committed fixtures and writes nothing.
"""

import json
import sys
import tokenize
from collections import defaultdict
from collections.abc import Callable, Sequence
from pathlib import Path
from types import SimpleNamespace

from fixture_files import write_or_check_files
from stdlib_fixtures import BUILDERS as STDLIB_FIXTURES

from agentless_mcp.core import clones, complexity, fileparse, grammars, normalize
from agentless_mcp.core.extractor import error_nodes
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import UnmeasurableSource

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "tests" / "characterization" / "fixtures"
FIXTURES = ROOT / "crates" / "metrics" / "fixtures"
# stdlib_fixtures.py writes tokenize.json, word.json and sum.json into the same directory.
SHARED = [ROOT / "crates" / path for path in STDLIB_FIXTURES]
DIRECTORY_LANGUAGES = {"py": "python"}

Case = dict[str, object]
Measure = Callable[[str, str, str], object]

# Each snippet is a file name and its lines. Directive and prose comments change the body keys,
# and the Python snippets reach the docstring, tokenizer and TYPE_CHECKING rules.
SNIPPETS: dict[str, list[tuple[str, tuple[str, ...]]]] = {
    "bash": [
        (
            "directives.sh",
            (
                "#!/usr/bin/env bash",
                "tagged() {",
                "  # shellcheck disable=SC2086",
                "  # noqa",
                "  echo $1",
                "  return 0",
                "}",
            ),
        ),
    ],
    "c": [
        (
            "directives.c",
            (
                "int tagged(int a) {",
                "    // fmt: off",
                "    /* NOLINT is prose */",
                "    if (a) { return 1; }",
                "    return a;",
                "}",
            ),
        ),
    ],
    "cpp": [
        (
            "directives.cpp",
            (
                "namespace outer {",
                "struct Holder {",
                "    int run(int a) {",
                "        // isort: skip",
                "        if (a) { return 1; }",
                "        return 0;",
                "    }",
                "};",
                "}",
                "int Holder::operator()(int a) { // pylint: disable",
                "    return a;",
                "}",
            ),
        ),
    ],
    "go": [
        (
            "directives.go",
            (
                "package main",
                "",
                "//go:noinline",
                "func tagged(a int) int {",
                "\t// +build linux",
                "\t// +buildx is prose",
                "\treturn a",
                "}",
                "",
                "func (r *Receiver) method() int {",
                "\t//go:generate stringer",
                "\tif r == nil {",
                "\t\treturn 0",
                "\t}",
                "\treturn 1",
                "}",
            ),
        ),
    ],
    "java": [
        (
            "Directives.java",
            (
                "class Tagged {",
                "    int run(int a) {",
                "        // noqa",
                "        // coverage: ignore",
                "        if (a > 0) { return 1; }",
                "        return 0;",
                "    }",
                "}",
            ),
        ),
    ],
    "javascript": [
        (
            "directives.js",
            (
                "function tagged(a) {",
                "  // eslint-disable-next-line no-console",
                "  console.log(a);",
                "  return a;",
                "}",
                "",
                "function prose(a) {",
                "  // a plain note",
                "  console.log(a);",
                "  return a;",
                "}",
                "",
                "const bound = (café) => {",
                "  /* @ts-ignore */",
                "  if (café) { return café; }",
                "  return null;",
                "};",
                "",
                'const holder = { method: function () { return 1; }, "quoted": () => 2 };',
            ),
        ),
    ],
    "lua": [
        (
            "directives.lua",
            (
                "local function tagged(a)",
                "  --!strict",
                "  -- luacheck: ignore",
                "  if a then return a end",
                "  return nil",
                "end",
                "",
                "M.handler = function(a)",
                "  --[[ block comment ]]",
                "  return a",
                "end",
            ),
        ),
    ],
    "python": [
        (
            "directives.py",
            (
                "def tagged(a):",
                "    # noqa: E501",
                "    # noqaX is prose",
                "    # noqa\N{COMBINING ACUTE ACCENT} ends in a combining mark",
                "    # yapf\N{ROMAN NUMERAL ONE} ends in a numeral",
                "    return a  # type: ignore[arg-type]",
                "",
                "",
                "def prose(a):",
                "    # an ordinary remark",
                "    return a",
            ),
        ),
        (
            "docstrings.py",
            (
                "def tupled():",
                '    "a",',
                "    return 1",
                "",
                "",
                "def prefixed():",
                '    b"bytes"',
                '    f"{1}"',
                '    rb"raw"',
                '    Rb"raw"',
                '    u"text"',
                "    return 2",
            ),
        ),
        (
            "form_feed.py",
            (
                "\x0c",
                "def shifted():",
                '    """Doc."""',
                "    if shifted:",
                "        return 1",
                "    return 2",
            ),
        ),
        (
            "type_checking.py",
            (
                "from typing import TYPE_CHECKING",
                "",
                "if TYPE_CHECKING:",
                "    import os",
                "    import re",
                "    import sys",
                "",
                "if typing.TYPE_CHECKING:",
                "    import os",
                "    import re",
                "    import sys",
            ),
        ),
        (
            "unicode.py",
            (
                "def café(naïve):",
                "    if naïve:",
                "        return naïve",
                '    x = "éé"; "after"',
                "    return None",
            ),
        ),
    ],
    "ruby": [
        (
            "directives.rb",
            (
                "# -*- coding: utf-8 -*-",
                "def tagged(a)",
                "  # mypy: ignore",
                "  # rubocop:disable Style",
                "  return a if a",
                "  nil",
                "end",
                "",
                "class Holder",
                "  def self.build(a)",
                "    # flake8: noqa",
                "    a",
                "  end",
                "end",
            ),
        ),
    ],
    "rust": [
        (
            "directives.rs",
            (
                "fn tagged(a: i32) -> i32 {",
                "    //! inner doc",
                "    // pragma: no cover",
                "    a",
                "}",
                "",
                "impl Trait for i32 {",
                "    fn owned(&self) -> i32 {",
                "        /* fmt: off */",
                "        if *self > 0 { 1 } else { 0 }",
                "    }",
                "}",
            ),
        ),
    ],
    "tsx": [
        (
            "directives.tsx",
            (
                "function View(props: { name: string }) {",
                "  // eslint-disable-line",
                "  if (props.name) { return <div>{props.name}</div>; }",
                "  return null;",
                "}",
            ),
        ),
    ],
    "typescript": [
        (
            "directives.ts",
            (
                "function tagged(a: number): number {",
                "  // @ts-expect-error",
                "  return a;",
                "}",
                "",
                "class Holder {",
                "  handler = (a: string): string => {",
                "    // prettier-ignore",
                "    if (a) { return a; }",
                "    return a;",
                "  };",
                "}",
                "",
                "abstract class Base {",
                "  run(): void { if (this) { return; } }",
                "}",
            ),
        ),
    ],
}


def main() -> int:
    """Write the fixtures, or compare them with --check, and return the exit status."""
    return write_or_check_files(__doc__.splitlines()[0], FIXTURES, fixtures, SHARED)


def fixtures() -> dict[Path, bytes]:
    contents = {
        FIXTURES / f"{language}.json": render([measured(language, *item) for item in items])
        for language, items in sorted(inputs().items())
    }
    contents[FIXTURES / "directives.json"] = render(directive_cases())
    contents[FIXTURES / "stamps.json"] = render(
        [[language, fileparse.metrics_stamp(language)] for language in grammars.ALL_LANGUAGES]
    )
    return contents


def inputs() -> dict[str, list[tuple[str, str]]]:
    found: dict[str, list[tuple[str, str]]] = defaultdict(list)
    for directory in sorted(CORPUS.glob("complexity_*")):
        suffix = directory.name.removeprefix("complexity_")
        language = DIRECTORY_LANGUAGES.get(suffix, suffix)
        for path in sorted(directory.iterdir()):
            source = path.read_bytes().decode("utf-8")
            found[language].append((path.name, source))
            found[language].append((f"{path.name}#half", source[: len(source) // 2]))
    for language, snippets in SNIPPETS.items():
        found[language].extend((name, "\n".join(lines) + "\n") for name, lines in snippets)
    return found


def measured(language: str, name: str, source: str) -> Case:
    case: Case = {"name": name, "source": source}
    refusal = refused(language, name, source)
    if refusal is not None:
        case["error"] = refusal
        return case
    units = complexity.measure(source, name, language)
    indexed = clones.file_blocks(source, name, language)
    case["units"] = [
        {
            "qualname": unit.qualname,
            "start_byte": unit.start_byte,
            "end_byte": unit.end_byte,
            "start_line": unit.start_line,
            "body_key": unit.body_key,
            "cc": unit.cc,
            "sloc": unit.sloc,
        }
        for unit in units
    ]
    case["blocks"] = [
        {
            "start_line": block.start_line,
            "end_line": block.end_line,
            "key": block.key,
            "exact": block.exact,
            "function": block.function,
            "tokens": block.tokens,
        }
        for block in indexed.blocks
    ]
    case["sloc_lines"] = list(indexed.sloc_lines)
    return case


def refused(language: str, name: str, source: str) -> list[object] | None:
    # Both measures must refuse an input the same way, or neither.
    sides: list[tuple[Measure, str, str]] = [
        (
            complexity.measure,
            MESSAGES.complexity_does_not_parse,
            MESSAGES.complexity_tokenizer_rejects,
        ),
        (clones.file_blocks, MESSAGES.clone_does_not_parse, MESSAGES.clone_tokenizer_rejects),
    ]
    found = [refusal(language, name, source, side) for side in sides]
    if found[0] != found[1]:
        message = f"{language} {name}: the measures refuse differently: {found}"
        raise SystemExit(message)
    return found[0]


def refusal(
    language: str, name: str, source: str, side: tuple[Measure, str, str]
) -> list[object] | None:
    measure, parse, tokenizer = side
    try:
        measure(source, name, language)
    except UnmeasurableSource as error:
        cause = error.__cause__
        if isinstance(cause, tokenize.TokenError | SyntaxError):
            reason = cause.args[0] if cause.args else ""
            kind = type(cause).__name__
            found: list[object] = ["tokenize", kind, reason]
            expected = tokenizer.format(path=name, detail=f"{kind}: {reason}")
        else:
            root = grammars.get_parser(language).parse(source.encode("utf-8")).root_node
            line = next(error_nodes(root)).start_point[0] + 1
            found = ["parse", line]
            expected = parse.format(path=name, line=line)
        if str(error) != expected:
            message = f"{language} {name}: the refusal {error} is not {expected}"
            raise SystemExit(message) from error
        return found
    return None


def directive_cases() -> list[list[str]]:
    # The function reads only the node's text, so a stand-in carries each comment's bytes.
    return [
        [comment, normalize._directive_text(SimpleNamespace(text=comment.encode("utf-8")))]
        for comment in directive_comments()
    ]


def directive_comments() -> list[str]:
    # After `noqa`: word characters, a combining mark, a numeral letter, a circled letter with
    # Other_Alphabetic, a code point unassigned in Unicode 15.1, and punctuation.
    after_word = (0x5F, 0x31, 0xE9, 0x301, 0x660, 0x2160, 0x24B6, 0x378, 0x2D, 0x3A)
    # Between the marker and the body: separators the whitespace split does and does not take.
    spaces = (0x1C, 0xA0, 0x2003, 0x200B, 0x3000)
    return [
        "",
        "   ",
        "#",
        "# noqa",
        "# noqa: E501",
        "#noqa",
        "# noqaX",
        "noqa",
        "comment without a marker: noqa",
        "// +build linux",
        "// +buildx",
        "//go:build linux",
        "// go:generate",
        "/* @ts-ignore */",
        "/*! inner */",
        "//! inner",
        "/// outer",
        "-- yapf: disable",
        "--yapfish",
        "--!strict",
        "#!/bin/sh",
        "# -*- coding: utf-8 -*-",
        "#  type:   ignore",
        "# TYPE: ignore",
        "# pragmatic",
        "# pragma: no cover",
        "# fmt: off",
        "# isort:skip",
        "# prettier-ignore",
        "// eslint-disable-next-line",
        "# ruff: noqa",
        "# pylint: disable=x",
        "# mypy: ignore-errors",
        "# flake8: noqa",
        "# coverage: ignore",
        "\t#   noqa  \n  more  ",
        "##  noqa",
        "# # noqa",
        *(f"# noqa{chr(code)}" for code in after_word),
        *(f"#{chr(code)}noqa" for code in spaces),
    ]


def render(cases: Sequence[object]) -> bytes:
    # One case per line, so a changed case shows as one changed line in a diff.
    lines = ",\n".join(f" {json.dumps(case)}" for case in cases)
    return ("[\n" + lines + "\n]\n").encode("ascii")


if __name__ == "__main__":
    sys.exit(main())
