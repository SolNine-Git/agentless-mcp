"""Write the patch crate's test fixtures: the inputs of the patch functions and what Python gives.

Development-time only. Run it from the repository root:

    uv run python scripts/patch_fixtures.py
    uv run python scripts/patch_fixtures.py --check

A pytest plugin records each call that four unit test modules make to the public functions of
patches, unidiff, normalize and vote. The script runs each call again with the unpatched
functions, and it keeps a call only when that result equals the recorded one, so a call that a
test patched stays out. The edge cases follow the recorded ones in each section. An expected
value is the text that json.dumps gives the result's as_dict form, so the Rust tests compare
bytes.
"""

import contextlib
import functools
import json
import re
import sys
import unicodedata
from collections.abc import Callable, Generator, Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any
from unittest import mock

import pytest
from checks_fixtures import unique
from fixture_files import Fixture, refuse_real_caches, write_or_check

from agentless_mcp.core import normalize, patches, patchlint, unidiff, vote
from agentless_mcp.core.patches import ApplyResult, BlockError, Edit, ParseResult
from agentless_mcp.core.unidiff import DiffParse
from agentless_mcp.core.vote import VoteCandidate, VoteReport

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "patch" / "fixtures"
UNIT_TESTS = ROOT / "tests" / "unit"
TEST_TARGETS = tuple(
    str(UNIT_TESTS / f"test_{name}.py") for name in ("patches", "unidiff", "normalize", "vote")
)

PARSE_BLOCKS = patches.parse_blocks
APPLY_EDITS = patches.apply_edits
RESOLVE_ELISIONS = patches.resolve_elisions
PARSE_UNIFIED_DIFF = unidiff.parse_unified_diff
ORIENTATION = unidiff.orientation
FILE_KEY = normalize.file_key
EQUIVALENCE_KEY = normalize.equivalence_key
NORMALIZED_STREAM = normalize.normalized_stream
SYNTAX_DELTA = normalize.syntax_delta
RANK = vote.rank

Json = dict[str, Any]


@dataclass
class Call:
    """One recorded call: its kind, the test that made it, its inputs, and what it gave."""

    kind: str
    node: str
    inputs: Json
    result: object = None


def dumped(value: object) -> str:
    """Return the text that json.dumps gives ``value`` with its default separators."""
    return json.dumps(value)


def edits_json(edits: Iterable[Edit]) -> list[Json]:
    return [edit.as_dict() for edit in edits]


def edits_of(rows: Iterable[Json]) -> list[Edit]:
    return [Edit(**row) for row in rows]


def parse_form(result: ParseResult) -> str:
    return dumped(result.as_dict())


def diff_form(parsed: DiffParse) -> Json:
    notes = [[note.path, note.reason] for note in parsed.notes]
    return {"result": dumped(parsed.result.as_dict()), "notes": notes}


def apply_form(result: ApplyResult) -> Json:
    return {"result": dumped(result.as_dict()), "new_contents": list(result.new_contents.items())}


def errors_form(errors: Iterable[BlockError]) -> str:
    return dumped([error.as_dict() for error in errors])


def report_form(report: VoteReport) -> str:
    return dumped(report.as_dict())


def candidates_json(candidates: Iterable[VoteCandidate]) -> list[Json]:
    return [
        {
            "id": entry.id,
            "index": entry.index,
            "applied": entry.applied,
            "measured": entry.measured,
            "equivalence_key": entry.equivalence_key,
            "regression_passed": entry.regression_passed,
            "reproduction_passed": entry.reproduction_passed,
        }
        for entry in candidates
    ]


def run(kind: str, inputs: Json) -> object:
    """Run the unpatched function of ``kind`` on ``inputs`` and return its result form."""
    runners: dict[str, Callable[[Json], object]] = {
        "parse_blocks": lambda given: parse_form(PARSE_BLOCKS(given["text"])),
        "apply_edits": lambda given: apply_form(
            APPLY_EDITS(edits_of(given["edits"]), given["contents"])
        ),
        "resolve_elisions": lambda given: list(elided(given)),
        "locate": lambda given: list(patchlint._locate(given["text"], given["block"])),
        "parse_unified_diff": lambda given: diff_form(PARSE_UNIFIED_DIFF(given["text"])),
        "orientation": lambda given: errors_form(
            ORIENTATION(edits_of(given["edits"]), given["texts"])
        ),
        "normalized_stream": lambda given: NORMALIZED_STREAM(given["text"], given["language"]),
        "file_key": lambda given: FILE_KEY(given["old"], given["new"], given["language"]),
        "equivalence_key": lambda given: changes_key(given["changes"]),
        "syntax_delta": lambda given: dumped(
            SYNTAX_DELTA(given["old"], given["new"], given["language"]).as_dict()
        ),
        "rank": lambda given: report_form(
            RANK(
                [VoteCandidate(**row) for row in given["candidates"]],
                repro_valid=given["repro_valid"],
            )
        ),
    }
    return runners[kind](inputs)


def elided(given: Json) -> tuple[str, str, str]:
    padded = "\n" + given["text"] + "\n"
    return RESOLVE_ELISIONS(given["search"], given["replace"], padded, ((0, len(padded)),))


def changes_key(changes: Sequence[tuple[str, str, str, str | None]]) -> str:
    table = {path: (old, new) for path, old, new, _ in changes}
    languages = {path: language for path, _, _, language in changes}
    return EQUIVALENCE_KEY(table, lambda path: languages[path])


class Recorder:
    """A pytest plugin that records each call to the wrapped functions during the tests."""

    def __init__(self) -> None:
        self.node = ""
        self.calls: list[Call] = []
        self.dropped: dict[str, int] = {}

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item: pytest.Item) -> Generator[None, None, None]:
        """Name the running test, so that each call records the test that made it."""
        self.node = item.nodeid
        yield
        self.node = ""

    def wrappers(self) -> list[tuple[ModuleType, str, Callable[..., Any]]]:
        """Return each module attribute that the recorder replaces, with its replacement."""
        return [
            (patches, "parse_blocks", self.parse_blocks),
            (patches, "apply_edits", self.apply_edits),
            (patches, "resolve_elisions", self.resolve_elisions),
            (unidiff, "parse_unified_diff", self.parse_unified_diff),
            (unidiff, "orientation", self.orientation),
            (normalize, "normalized_stream", self.normalized_stream),
            (normalize, "file_key", self.file_key),
            (normalize, "equivalence_key", self.equivalence_key),
            (normalize, "syntax_delta", self.syntax_delta),
            (vote, "rank", self.rank),
        ]

    def keep(self, kind: str, inputs: Json, result: object) -> None:
        self.calls.append(Call(kind, self.node, inputs, result))

    def drop(self, reason: str) -> None:
        self.dropped[reason] = self.dropped.get(reason, 0) + 1

    def parse_blocks(self, text: str) -> ParseResult:
        result = PARSE_BLOCKS(text)
        self.keep("parse_blocks", {"text": text}, parse_form(result))
        return result

    def apply_edits(self, edits: Sequence[Edit], file_contents: Mapping[str, str]) -> ApplyResult:
        inputs = {"edits": edits_json(edits), "contents": dict(file_contents)}
        result = APPLY_EDITS(edits, file_contents)
        self.keep("apply_edits", inputs, apply_form(result))
        return result

    def resolve_elisions(
        self, search: str, replace: str, padded: str, scopes: Sequence[tuple[int, int]]
    ) -> tuple[str, str, str]:
        result = RESOLVE_ELISIONS(search, replace, padded, scopes)
        whole = tuple(scopes) == ((0, len(padded)),)
        if whole and len(padded) > 1 and padded[0] == padded[-1] == "\n":
            inputs = {"search": search, "replace": replace, "text": padded[1:-1]}
            self.keep("resolve_elisions", inputs, list(result))
        else:
            self.drop("a scope other than the whole padded file")
        return result

    def parse_unified_diff(self, text: str) -> DiffParse:
        parsed = PARSE_UNIFIED_DIFF(text)
        self.keep("parse_unified_diff", {"text": text}, diff_form(parsed))
        return parsed

    def orientation(
        self, edits: Sequence[Edit], texts: Mapping[str, str]
    ) -> tuple[BlockError, ...]:
        inputs = {"edits": edits_json(edits), "texts": dict(texts)}
        problems = ORIENTATION(edits, texts)
        self.keep("orientation", inputs, errors_form(problems))
        return problems

    def normalized_stream(self, text: str, language: str | None) -> str:
        stream = NORMALIZED_STREAM(text, language)
        self.keep("normalized_stream", {"text": text, "language": language}, stream)
        return stream

    def file_key(self, old: str, new: str, language: str | None) -> str:
        key = FILE_KEY(old, new, language)
        self.keep("file_key", {"old": old, "new": new, "language": language}, key)
        return key

    def equivalence_key(
        self,
        changes: Mapping[str, tuple[str, str]],
        language_of: Callable[[str], str | None],
    ) -> str:
        rows = [[path, old, new, language_of(path)] for path, (old, new) in changes.items()]
        key = EQUIVALENCE_KEY(changes, language_of)
        self.keep("equivalence_key", {"changes": rows}, key)
        return key

    def syntax_delta(self, old: str, new: str, language: str | None) -> normalize.SyntaxVerdict:
        verdict = SYNTAX_DELTA(old, new, language)
        inputs = {"old": old, "new": new, "language": language}
        self.keep("syntax_delta", inputs, dumped(verdict.as_dict()))
        return verdict

    def rank(self, candidates: Sequence[VoteCandidate], *, repro_valid: bool) -> VoteReport:
        inputs = {"candidates": candidates_json(candidates), "repro_valid": repro_valid}
        report = RANK(candidates, repro_valid=repro_valid)
        self.keep("rank", inputs, report_form(report))
        return report


@functools.cache
def recorded() -> tuple[Call, ...]:
    """Run the four test modules with the recorder installed, and return the calls it kept."""
    recorder = Recorder()
    with contextlib.ExitStack() as installed:
        for module, name, wrapper in recorder.wrappers():
            installed.enter_context(mock.patch.object(module, name, wrapper))
        arguments = ["-q", "-p", "no:cacheprovider", "--rootdir", str(ROOT), *TEST_TARGETS]
        status = pytest.main(arguments, plugins=[recorder])
    if status != 0:
        message = f"the recorded tests failed with pytest exit status {status}"
        raise SystemExit(message)
    kept = []
    for call in recorder.calls:
        if run(call.kind, call.inputs) == call.result:
            kept.append(call)
        else:
            recorder.drop("a patched dependency")
    print(f"recorded {len(recorder.calls)} calls, kept {len(kept)}, dropped {recorder.dropped}")
    return tuple(kept)


def section(kind: str, edges: Iterable[tuple[str, Json]]) -> list[object]:
    """Return the recorded cases of ``kind``, then the edge cases, each with its expected value."""
    cases = [
        {"name": call.node, **call.inputs, "expect": call.result}
        for call in recorded()
        if call.kind == kind
    ]
    cases.extend(
        {"name": f"edge: {name}", **inputs, "expect": run(kind, inputs)} for name, inputs in edges
    )
    return list(unique(cases))


def block(path: str, search: str, replace: str) -> str:
    return f"{path}\n<<<<<<< SEARCH\n{search}\n=======\n{replace}\n>>>>>>> REPLACE\n"


HEADERS = (
    "src/app.py",
    "### src/app.py",
    "# src/app.py",
    "###",
    "```python",
    "```",
    "Done!",
    "Next:",
    "Also,",
    "Fixed.",
    "Makefile",
    "src/na\U000000efve.py",
    "\U00006570\U0000636e.py",
    "../../etc/passwd",
    '"quoted.py"',
    "'single.py'",
    "(paren.py",
    "[it's.py",
    "{brace\x07.py",
    "\"mixed 'quotes'.py",
    "my file.py",
    "a b c.py",
    "a b c d.py",
    "a  b.py",
    "a\tb.py",
    "now fixing",
    "a b.\U000000efx",
    "a b.\U00002460",
    "a b.\U0000216b",
    "a b.x_y",
    "a b.abcdefghijkl",
    "a b.abcdefghijklm",
    "a b.",
    "dir name/.py",
    "a b/c d.py",
    "a.b c",
    "\x1c",
    "  src/indented.py  ",
    "\U00003000src/ideographic.py\U00003000",
)


def blocks_edges() -> list[tuple[str, Json]]:
    edges: list[tuple[str, Json]] = [
        (f"header {header!r}", {"text": block(header, "a", "b")}) for header in HEADERS
    ]
    bare = block("", "a", "b")[1:]
    texts = {
        "only whitespace": "   \n\t\n",
        "prose then real header": "src/a.py\nI will fix it.\n" + bare,
        "header after a line separator": "Intro.\U00002028src/a.py\n" + bare,
        "header after NEL": "Intro.\x85src/a.py\n" + bare,
        "header after VT and FF": "x\x0bsrc/a.py\x0c\n" + bare,
        "crlf header lines": "src/a.py\r\n" + bare,
        "two search markers": "src/a.py\n<<<<<<< SEARCH\n<<<<<<< SEARCH\na\n=======\nb\n"
        ">>>>>>> REPLACE\n",
        "three search markers": "a.py\n<<<<<<< SEARCH<<<<<<< SEARCH<<<<<<< SEARCH\n"
        ">>>>>>> REPLACE\n",
        "text after the search marker": "a.py\n<<<<<<< SEARCH extra\na\n=======\nb\n"
        ">>>>>>> REPLACE\n",
        "spaces after the search marker": "a.py\n<<<<<<< SEARCH   \na\n=======\nb\n"
        ">>>>>>> REPLACE\n",
        "crlf search marker": "a.py\r\n<<<<<<< SEARCH\r\na\r\n=======\r\nb\r\n>>>>>>> REPLACE\r\n",
        "crlf divider only": "a.py\n<<<<<<< SEARCH\na\n=======\r\nb\n>>>>>>> REPLACE\n",
        "cr inside content": "a.py\n<<<<<<< SEARCH\na\r\n=======\nb\r\n>>>>>>> REPLACE\n",
        "divider with trailing spaces": "a.py\n<<<<<<< SEARCH\na\n=======  \nb\n>>>>>>> REPLACE\n",
        "divider with a leading space": "a.py\n<<<<<<< SEARCH\na\n =======\nb\n>>>>>>> REPLACE\n",
        "three dividers": "a.py\n<<<<<<< SEARCH\n=======\n=======\n=======\n>>>>>>> REPLACE\n",
        "content on the closing line": "a.py\n<<<<<<< SEARCH\na\n=======\nb>>>>>>> REPLACE\n",
        "empty sides": "a.py\n<<<<<<< SEARCH\n=======\n>>>>>>> REPLACE\n",
        "no newline before divider": "a.py\n<<<<<<< SEARCH=======\n>>>>>>> REPLACE",
        "elided replace with spaces": "a.py\n<<<<<<< SEARCH\n...\n=======\n  ...  \n"
        ">>>>>>> REPLACE\n",
        "elided search, real replace": "a.py\n<<<<<<< SEARCH\n...\n=======\n...\nx\n"
        ">>>>>>> REPLACE\n",
        "unterminated naming its own file": block("a.py", "a", "b")
        + "b.py\n<<<<<<< SEARCH\nc\n=======\n",
        "unterminated with an unreadable header": block("a.py", "a", "b")
        + "'q.py'\n<<<<<<< SEARCH\nc\n",
        "unterminated with no path": "<<<<<<< SEARCH\nc\n",
        "unreadable header inherits for its error": block("a.py", "a", "b")
        + block('"b.py"', "c", "d"),
        "inherited path": block("a.py", "a", "b") + bare.replace("a\n", "c\n", 1),
        "replace marker only": ">>>>>>> REPLACE",
        "two replace markers": ">>>>>>> REPLACE>>>>>>> REPLACE",
        "text then a replace marker": "foo\n>>>>>>> REPLACE\n",
        "non-ASCII content": block("a.py", "\U000000e9t\U000000e9", "\U0001f600"),
        "begin patch with a block": "*** Begin Patch\n" + block("a.py", "a", "b"),
        "begin patch alone, unterminated search": "*** Begin Patch\n<<<<<<< SEARCH\n",
        "fence close and next fence": "```python\n### a.py\n<<<<<<< SEARCH\na\n=======\nb\n"
        ">>>>>>> REPLACE\n```\n\n```\n<<<<<<< SEARCH\nc\n=======\nd\n>>>>>>> REPLACE\n```\n",
    }
    edges.extend((name, {"text": text}) for name, text in texts.items())
    return edges


def diff(*lines: str) -> str:
    return "".join(f"{line}\n" for line in lines)


SIMPLE_HEAD = ("diff --git a/app.py b/app.py", "--- a/app.py", "+++ b/app.py")
HUNK = ("@@ -1,2 +1,2 @@", " keep", "-old", "+new")


def header_texts() -> dict[str, str]:
    """Return the diffs whose sections differ in their headers."""
    return {
        "empty": "",
        "no sections": "just prose\n",
        "dashes alone do not open a section": diff("--- ", "+++ b/x", "@@ -1 +1 @@", "-a", "+b"),
        "combined": diff("diff --cc app.py", "@@@ -1,1 -1,1 +1,1 @@@"),
        "combined after a section": diff(*SIMPLE_HEAD, *HUNK, "diff --combined x"),
        "crlf diff git line": "diff --git a/x b/x\r\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n",
        "crlf old line": "--- a/x\r\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n",
        "crlf hunk header": "--- a/x\n+++ b/x\n@@ -1 +1 @@\r\n-a\n+b\n",
        "cr in content": "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\r\n+b\r\n",
        "binary literal": diff("diff --git a/i.png b/i.png", "GIT binary patch", "literal 5"),
        "binary text": diff(
            "diff --git a/i.png b/i.png", "Binary files a/i.png and b/i.png differ"
        ),
        "binary plain": diff("Binary files a/i.png and b/i.png differ"),
        "rename headers": diff(
            "diff --git a/old.py b/new.py",
            "similarity index 90%",
            "rename from old.py",
            "rename to new.py",
        ),
        "copy headers with hunks": diff(
            "diff --git a/old.py b/new.py",
            "copy from old.py",
            "copy to new.py",
            "--- a/old.py",
            "+++ b/new.py",
            *HUNK,
        ),
        "rename by paths": diff("--- a/old.py", "+++ b/new.py", *HUNK),
        "same raw both sides": diff("--- a/x.py", "+++ a/x.py", *HUNK),
        "ignored headers": diff(
            "diff --git a/x b/x",
            "old mode 100644",
            "new mode 100755",
            "index 1..2 100644",
            "--- a/x",
            "+++ b/x",
            *HUNK,
        ),
        "new and deleted file modes": diff(
            "diff --git a/n b/n",
            "new file mode 100644",
            "--- /dev/null",
            "+++ b/n",
            "@@ -0,0 +1,2 @@",
            "+one",
            "+two",
            "diff --git a/d b/d",
            "deleted file mode 100644",
            "--- a/d",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-gone",
        ),
        "unrecognised header": diff("diff --git a/x b/x", "weird \U000000fc\x07 header", *HUNK),
        "mode only": diff("diff --git a/x.sh b/x.sh", "old mode 100644", "new mode 100755"),
        "mode only then a section": diff(
            "diff --git a/x.sh b/x.sh",
            "old mode 100644",
            "new mode 100755",
            "diff --git a/y b/y",
            "--- a/y",
            "+++ b/y",
            *HUNK,
        ),
        "hunks without a pair": diff("diff --git a/x b/x", *HUNK),
        "quoted diff git and mode only": diff(
            'diff --git "a/q x" "b/q x"', "old mode 100644", "new mode 100755"
        ),
        "only the old header": diff("diff --git a/x b/x", "--- a/x", *HUNK),
        "only the new header": diff("diff --git a/x b/x", "+++ b/x", *HUNK),
        "c-quoted path": diff('--- "a/na\\303\\257ve.py"', '+++ "b/na\\303\\257ve.py"', *HUNK),
        "dev null both sides": diff("--- /dev/null", "+++ /dev/null", *HUNK),
        "empty path": diff("--- a/", "+++ b/", *HUNK),
        "timestamps": diff("--- a/x.py\t2020-01-01 00:00", "+++ b/x.py\t2020-01-02", *HUNK),
        "trailing space in path": diff("--- a/x.py ", "+++ b/x.py ", *HUNK),
        "no prefix": diff("--- x.py", "+++ x.py", *HUNK),
        "header but no hunks": diff("--- a/x", "+++ b/x"),
        "header then prose": diff("--- a/x", "+++ b/x", "prose"),
        "diff git label with b/ in the path": diff(
            "diff --git a/a b/c b/a b/c", "old mode 100644", "new mode 100755"
        ),
        "diff git label that disagrees": diff(
            "diff --git a/a b/c b/d b/e", "old mode 100644", "new mode 100755"
        ),
        "diff git label with no b/": diff("diff --git a/x c/x", "old mode 1", "new mode 2"),
        "diff git label with an empty tail": diff("diff --git a/x b/", "old mode 1", "new mode 2"),
        "non-ASCII paths": diff(
            "--- a/na\U000000efve.py",
            "+++ b/na\U000000efve.py",
            "@@ -1 +1 @@",
            "-\U000000e9",
            "+\U0001f600",
        ),
        "second old line ends the section": diff("--- a/x", "--- a/y", "+++ b/y", *HUNK),
    }


def hunk_texts() -> dict[str, str]:
    """Return the diffs whose sections differ in their hunks."""
    head = ("--- a/x", "+++ b/x")
    return {
        "malformed hunk header": diff(*head, "@@ -a +b @@ \U000000e9", "-a"),
        "hunk header comma without digits": diff(*head, "@@ -1, +1 @@", "-a"),
        "hunk header with context": diff(*head, "@@ -1 +1 @@ def f():", "-a", "+b"),
        "no lines either side": diff(*head, "@@ -0,0 +0,0 @@"),
        "zero context into existing": diff(*head, "@@ -3,0 +4,1 @@", "+added"),
        "zero context into new": diff("--- /dev/null", "+++ b/x", "@@ -0,0 +1 @@", "+added"),
        "overlapping hunks": diff(
            *head, "@@ -5,2 +5,2 @@", " a", "-b", "+B", "@@ -6,1 +6,1 @@", "-c", "+C"
        ),
        "leading zeros in an overlap": diff(
            *head, "@@ -005,2 +005,2 @@", " a", "-b", "+B", "@@ -0006,1 +6 @@", "-c", "+C"
        ),
        "omitted counts": diff(*head, "@@ -3 +3 @@", "-a", "+b"),
        "arabic-indic digits": diff(
            *head, "@@ -\U00000661,\U00000662 +\U00000661,\U00000662 @@", " a", "-b", "+B"
        ),
        "fullwidth and math digits in an overlap": diff(
            *head,
            "@@ -\U0000ff15,\U0000ff12 +5,2 @@",
            " a",
            "-b",
            "+B",
            "@@ -\U0001d7d4\U0001d7d0 +1 @@",
            "-c",
            "+C",
        ),
        "a huge pre-image count": diff(
            *head, "@@ -1,99999999999999999999999999 +1,1 @@", " a", "-b"
        ),
        "a huge post-image count": diff(
            *head, "@@ -1,1 +1,123456789012345678901234567890 @@", " a", "+b"
        ),
        "a huge start then an overlap": diff(
            *head,
            "@@ -99999999999999999999,1 +1,1 @@",
            "-a",
            "+b",
            "@@ -99999999999999999999,1 +1,1 @@",
            "-c",
            "+d",
        ),
        "a huge start then a later one": diff(
            *head,
            "@@ -99999999999999999999,1 +1,1 @@",
            "-a",
            "+b",
            "@@ -100000000000000000000,1 +2,1 @@",
            "-c",
            "+d",
        ),
        "a huge start below the floor": diff(
            *head,
            "@@ -18446744073709551615,2 +1,1 @@",
            "-a",
            "-b",
            "+c",
            "@@ -18446744073709551616,1 +2,1 @@",
            "-c",
            "+d",
        ),
        "truncated body": diff(*head, "@@ -1,3 +1,3 @@", " a"),
        "body line with a bad marker": diff(*head, "@@ -1 +1 @@", "?x", "+b"),
        "body line with a non-ASCII marker": diff(*head, "@@ -1 +1 @@", "\U000000fcx", "+b"),
        "no newline markers": diff(
            *head,
            "@@ -1 +1 @@",
            "-a",
            "\\ No newline at end of file",
            "+b",
            "\\ No newline at end of file",
        ),
        "empty line as context": diff(*head, "@@ -1,3 +1,3 @@", " a", "", "-b", "+c"),
        "added line past the count": diff(*head, "@@ -1,1 +1,1 @@", "+a", "+b"),
        "removed line past the count": diff(*head, "@@ -1,1 +1,1 @@", "-a", "-b"),
        "context past the count": diff(*head, "@@ -1,1 +1,2 @@", "+a", " b"),
        "stray body after the counts": diff(*head, "@@ -1 +1 @@", "-a", "+b", "+c"),
        "format patch trailer": diff(*SIMPLE_HEAD, *HUNK, "-- ", "2.40.0", ""),
        "trailer then a hidden hunk": diff(
            *SIMPLE_HEAD, "@@ -1 +1 @@", "-a", "+b", "--", "@@ -9 +9 @@", "-c", "+d"
        ),
        "blank then stray body": diff(*SIMPLE_HEAD, "@@ -1 +1 @@", "-a", "+b", "", "+c"),
        "blank then a commit header": diff(
            *SIMPLE_HEAD, "@@ -1 +1 @@", "-a", "+b", "", "commit 0123", "    message"
        ),
        "prologue": diff(
            "From 0123 Mon Sep 17 00:00:00 2001",
            "Subject: fix",
            "",
            "---",
            " app.py | 2 +-",
            "",
            *SIMPLE_HEAD,
            *HUNK,
        ),
        "two sections": diff(*SIMPLE_HEAD, *HUNK, "--- a/b.py", "+++ b/b.py", *HUNK),
        "content that reads like a header": diff(
            *head, "@@ -1,2 +1,2 @@", "--- a/y", "-old", "+new", "++++ z"
        ),
        "a refused git section resumes at the next one": diff(
            "diff --git a/x b/x",
            "weird",
            *head,
            "@@ -1 +1 @@",
            "--- a/q",
            "+b",
            "diff --git a/y b/y",
            "--- a/y",
            "+++ b/y",
            *HUNK,
        ),
        "a refused plain section resumes at the next one": diff(
            *head, "@@ -1 +1 @@", "?bad", "--- a/y", "+++ b/y", *HUNK
        ),
        "an unreadable hunk header in a refused section": diff(
            "diff --git a/x b/x",
            "weird",
            "@@ bad @@",
            "--- a/x",
            "diff --git a/y b/y",
            "--- a/y",
            "+++ b/y",
            *HUNK,
        ),
        "no final newline": "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b",
    }


def unidiff_edges() -> list[tuple[str, Json]]:
    texts = {**header_texts(), **hunk_texts()}
    return [(name, {"text": text}) for name, text in texts.items()]


def edit_row(search: str, replace: str, path: str = "a.py", index: int = 0) -> Json:
    return Edit(index, path, search, replace).as_dict()


def orientation_edges() -> list[tuple[str, Json]]:
    rows = [
        ("no edits", [], {"a.py": "x\n"}),
        ("pre-image present", [edit_row("x", "y")], {"a.py": "x\n"}),
        ("post-image present", [edit_row("x", "y")], {"a.py": "y\n"}),
        ("neither present", [edit_row("x", "y")], {"a.py": "z\n"}),
        ("empty replace, absent search", [edit_row("x", "")], {"a.py": "z\n"}),
        ("added file exists", [edit_row("", "y", "n\U000000e9.py", 3)], {"n\U000000e9.py": ""}),
        ("added file absent", [edit_row("", "y", "n.py")], {}),
        ("text with no newline", [edit_row("x", "y")], {"a.py": "x"}),
        ("multi-line pre-image", [edit_row("a\nb", "c")], {"a.py": "q\na\nb\n"}),
        (
            "two files, each reported once",
            [
                edit_row("x", "y", "a.py", 0),
                edit_row("u", "v", "a.py", 1),
                edit_row("p", "q", "b.py", 2),
                edit_row("s", "t", "b.py", 5),
                edit_row("m", "n", "c.py", -1),
            ],
            {"a.py": "z\n", "b.py": "q\n", "c.py": "m\n"},
        ),
    ]
    return [(name, {"edits": edits, "texts": texts}) for name, edits, texts in rows]


SOURCE = "alpha\nbravo\ncharlie\n"
ONE = {"a.py": SOURCE}


def elision_apply_rows() -> list[tuple[str, list[Json], dict[str, str]]]:
    """Return the apply cases that exercise the ``...`` elision."""
    return [
        ("bare elision at the top", [edit_row("...", "import os")], ONE),
        ("bare elision, quote in the anchor", [edit_row("...", "x")], {"a.py": "it's\n  b\n"}),
        (
            "bare elision, non-ASCII anchor",
            [edit_row("...", "x")],
            {"a.py": "  a\n\U000000e9\x07\n"},
        ),
        ("bare elision deep in a file", [edit_row("...", "x")], {"a.py": "  a\n  b\n\n\nlast\n"}),
        ("bare elision, last line has no newline", [edit_row("...", "x")], {"a.py": " a\nend"}),
        ("bare elision, indented replace", [edit_row("...", " x")], ONE),
        ("bare elision, tab replace", [edit_row("...", "\tx")], ONE),
        ("bare elision, NBSP replace", [edit_row("...", "\U000000a0x")], ONE),
        ("bare elision, empty replace", [edit_row("...", "")], ONE),
        ("bare elision, replace is the prefix", [edit_row("...", "...\n")], ONE),
        ("bare elision, prefixed replace", [edit_row("...", "...\nimport os")], ONE),
        ("bare elision, prefixed indented replace", [edit_row("...", "...\n  x")], ONE),
        ("bare elision, no anchor", [edit_row("...", "x")], {"a.py": "  a\n  b\n"}),
        ("bare elision, only duplicates", [edit_row("...", "x")], {"a.py": "a\na\n b\n"}),
        ("bare elision on an empty file", [edit_row("...", "x")], {"a.py": ""}),
        ("bare elision, ideographic space", [edit_row("...", "x")], {"a.py": "\U00003000a\nb\n"}),
        ("leading elision on search", [edit_row("...\nbravo", "BRAVO")], ONE),
        ("search is only the prefix", [edit_row("...\n", "X")], {"a.py": "...\n\nz\n"}),
        ("leading elision on replace", [edit_row("bravo", "...\nBRAVO")], ONE),
        ("replace is only the prefix", [edit_row("bravo", "...\n")], ONE),
    ]


def plain_apply_rows() -> list[tuple[str, list[Json], dict[str, str]]]:
    """Return the apply cases without an elision."""
    return [
        ("no edits", [], ONE),
        ("empty search", [edit_row("", "x")], ONE),
        ("empty search on an empty file", [edit_row("", "x")], {"a.py": ""}),
        ("overlapping matches", [edit_row("x", "y")], {"a.py": "x\nx\nx\n"}),
        ("multi-line overlap", [edit_row("x\nx", "y")], {"a.py": "x\nx\nx\n"}),
        ("whole-line only", [edit_row("rav", "X")], ONE),
        ("first line", [edit_row("alpha", "A")], ONE),
        ("last line without a newline", [edit_row("c", "C")], {"a.py": "a\nb\nc"}),
        ("search to the end", [edit_row("bravo\ncharlie\n", "X\n")], ONE),
        ("replace with nothing", [edit_row("bravo", "")], ONE),
        ("same text", [edit_row("bravo", "bravo")], ONE),
        ("crlf file, lf search", [edit_row("a", "b")], {"a.py": "a\r\nb\r\n"}),
        ("crlf file, cr search", [edit_row("a\r", "z\r")], {"a.py": "a\r\nb\r\n"}),
        ("empty file", [edit_row("x", "y")], {"a.py": ""}),
        ("non-ASCII", [edit_row("\U000000e9", "\U0001f600")], {"a.py": "\U000000e9\n\U00006570\n"}),
        (
            "edits in order across files",
            [
                edit_row("alpha", "A", "b.py", 0),
                edit_row("x", "y", "missing.py", 1),
                edit_row("A", "AA", "b.py", 2),
                edit_row("bravo", "B", "a.py", 3),
                edit_row("nope", "N", "a.py", 4),
                edit_row("charlie", "C", "c.py", 5),
            ],
            {"a.py": SOURCE, "b.py": SOURCE, "c.py": SOURCE},
        ),
        ("a change and its undo", [edit_row("bravo", "B"), edit_row("B", "bravo")], ONE),
        ("a failed edit beside an applied one", [edit_row("bravo", "B"), edit_row("zz", "q")], ONE),
        (
            "an ambiguity beside an applied one",
            [edit_row("x", "y"), edit_row("q", "Q")],
            {"a.py": "x\nx\nq\n"},
        ),
    ]


def apply_edges() -> list[tuple[str, Json]]:
    rows = plain_apply_rows() + elision_apply_rows()
    return [(name, {"edits": edits, "contents": contents}) for name, edits, contents in rows]


def elision_edges() -> list[tuple[str, Json]]:
    rows = [
        ("plain", "a", "b", "a\n"),
        ("bare", "...", "x", "a\n"),
        ("bare with empty text", "...", "x", ""),
        ("bare with prefixed replace", "...", "...\nx", "a\nb\n"),
        ("bare with space replace", "...", " x", "a\n"),
        ("bare with no anchor", "...", "x", " a\n a\n"),
        ("anchor first unique", "...", "x", "a\nb\na\n"),
        ("search prefix", "...\nq", "x", ""),
        ("search prefix only", "...\n", "x", ""),
        ("replace prefix", "q", "...\nx", ""),
        ("replace prefix only", "q", "...\n", ""),
        ("double prefix", "...\n...\nq", "...\n...\nx", ""),
        ("dots inside", "a...", "...b", ""),
    ]
    return [
        (name, {"search": search, "replace": replace, "text": text})
        for name, search, replace, text in rows
    ]


def locate_edges() -> list[tuple[str, Json]]:
    rows = [
        ("absent", "a\nb\n", "c"),
        ("first line", "a\nb\n", "a"),
        ("last line, no newline", "a\nb", "b"),
        ("multi-line", "a\nb\nc\n", "b\nc"),
        ("block with a trailing newline", "a\nb\n\nc\n", "b\n"),
        ("ambiguous", "a\na\n", "a"),
        ("ambiguous by overlap", "a\na\na\n", "a\na"),
        ("substring only", "abc\n", "b"),
        ("empty text", "", "a"),
        ("empty block", "a\n\nb\n", ""),
        ("both empty", "", ""),
        ("non-ASCII", "\U000000e9\n\U0001f600\nz\n", "\U0001f600"),
        ("deep", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n", "9\n10"),
        ("crlf", "a\r\nb\r\n", "b\r"),
    ]
    edges = [(name, {"text": text, "block": block}) for name, text, block in rows]
    for name, inputs in apply_edges():
        for row in inputs["edits"]:
            text = inputs["contents"].get(row["path"])
            if text is not None:
                edges.append((f"apply {name}", {"text": text, "block": row["search"]}))
    return edges


# One clean snippet and one broken snippet for each of the 22 grammars.
SNIPPETS = {
    "bash": ("x=1\nif [ $x ]; then echo hi; fi\n", "if [ $x ]; then echo hi\n"),
    "c": ("int f(void) { return 1; }\n", "int f(void) { return 1;\n"),
    "cpp": ("int f() { return 1; }\n", "int f() { return 1;\n"),
    "csharp": ("class A { int F() { return 1; } }\n", "class A { int F() { return 1; }\n"),
    "go": ("package a\n\nfunc F() int { return 1 }\n", "package a\n\nfunc F() int { return\n"),
    "hcl": ('a = "b"\nblock "x" {\n  y = 1\n}\n', 'block "x" {\n  y = \n'),
    "java": ("class A { int f() { return 1; } }\n", "class A { int f() { return 1; }\n"),
    "javascript": ("function f() { return 1; }\n", "function f() { return 1;\n"),
    "json": ('{"a": [1, 2]}\n', '{"a": [1, 2}\n'),
    "kotlin": ("fun f(): Int { return 1 }\n", "fun f(): Int { return 1\n"),
    "lua": ("local function f() return 1 end\n", "local function f() return 1\n"),
    "php": ("<?php\nfunction f() { return 1; }\n", "<?php\nfunction f() { return 1;\n"),
    "python": ("def f():\n    return 1\n", "def f(:\n    return 1\n"),
    "ruby": ("def f\n  1\nend\n", "def f\n  1\n"),
    "rust": ("fn f() -> i32 { 1 }\n", "fn f() -> i32 { 1\n"),
    "scala": ("object A { def f(): Int = 1 }\n", "object A { def f(): Int = }\n"),
    "sql": ("SELECT a FROM t WHERE b = 1;\n", "SELECT a FROM WHERE;\n"),
    "swift": ("func f() -> Int { return 1 }\n", "func f() -> Int { return 1\n"),
    "toml": ("[a]\nb = 1\n", "[a\nb = \n"),
    "tsx": ("const A = () => <div>{1}</div>;\n", "const A = () => <div>{1</div>;\n"),
    "typescript": (
        "function f(x: number): number { return x; }\n",
        "function f(x: number: number { return x; }\n",
    ),
    "yaml": ("a:\n  - 1\n  - 2\n", "a: [1, 2\n"),
}

TEXTS = {
    "empty": "",
    "only whitespace": " \t\n\x0b\x0c\r\x1c\x1d\x1e\x1f\x85\U000000a0\U00003000\U00002028",
    "words": "hello   world\n\tagain\x1cfs\U00003000ideo",
    "line breaks in a token": 'x = "a\U00002028b\x85c"\n',
    "crlf": "a = 1\r\nb = 2\r\n",
}

DIRECTIVES = {
    "python": (
        "x = 1  # type: ignore\n#!/usr/bin/env python\n# noqa: E501\n# prose\n# pragmatic\n"
        "# ruff: noqa\n# fmt: off\n#    pylint:   disable=x\n# yapf: disable\n# noqaz\n"
    ),
    "go": "//go:build linux\n// +build linux\n// +buildx\n/* eslint-disable */\npackage a\n",
    "typescript": "// @ts-expect-error\nconst a = 1; // prettier-ignore\n/** prose */\n",
    "lua": "-- -*- coding: utf-8 -*-\nlocal a = 1 -- isort: skip\n",
    "sql": "-- mypy: ignore\nSELECT 1;\n",
}


def stream_edges() -> list[tuple[str, Json]]:
    edges: list[tuple[str, Json]] = []
    for language, (clean, broken) in SNIPPETS.items():
        edges.append((f"{language} clean", {"text": clean, "language": language}))
        edges.append((f"{language} broken", {"text": broken, "language": language}))
    for language, text in DIRECTIVES.items():
        edges.append((f"{language} directives", {"text": text, "language": language}))
    for name, text in TEXTS.items():
        edges.append((f"text {name}", {"text": text, "language": None}))
        edges.append((f"python {name}", {"text": text, "language": "python"}))
    return edges


def file_key_edges() -> list[tuple[str, Json]]:
    edges: list[tuple[str, Json]] = []
    for language, (clean, broken) in SNIPPETS.items():
        edges.append((f"{language} same", {"old": clean, "new": clean, "language": language}))
        edges.append((f"{language} broken", {"old": clean, "new": broken, "language": language}))
    for name, text in TEXTS.items():
        edges.append((f"text {name}", {"old": "a b\n", "new": text, "language": None}))
        edges.append((f"python {name}", {"old": "a = 1\n", "new": text, "language": "python"}))
    edges.append(("both empty", {"old": "", "new": "", "language": None}))
    long_old = "".join(f"v{n} = {n}\n" for n in range(40))
    long_new = long_old.replace("v7 = 7", "v7 = 8").replace("v30 = 30", "v30 = 31")
    edges.append(("two hunks", {"old": long_old, "new": long_new, "language": "python"}))
    return edges


def equivalence_edges() -> list[tuple[str, Json]]:
    py = SNIPPETS["python"]
    go = SNIPPETS["go"]
    rows = [
        ("no changes", []),
        ("one python file", [["a.py", py[0], py[1], "python"]]),
        ("one text file", [["a.txt", "a\n", "b\n", None]]),
        (
            "mixed and unsorted",
            [
                ["z.go", go[0], go[1], "go"],
                ["\U000000e9.py", py[0], py[1], "python"],
                ["a.py", py[0], py[0], "python"],
                ["A.txt", "x", "y", None],
                ["\U0001f600.py", py[1], py[0], "python"],
            ],
        ),
        ("all with grammars", [["b.go", go[0], go[1], "go"], ["a.py", py[0], py[1], "python"]]),
    ]
    return [(name, {"changes": changes}) for name, changes in rows]


def syntax_delta_edges() -> list[tuple[str, Json]]:
    edges: list[tuple[str, Json]] = []
    for language, (clean, broken) in SNIPPETS.items():
        for name, old, new in (
            ("clean", clean, clean),
            ("broke", clean, broken),
            ("fixed", broken, clean),
            ("still broken", broken, broken),
            ("broken twice", clean, broken + broken),
        ):
            edges.append((f"{language} {name}", {"old": old, "new": new, "language": language}))
    edges.append(("no language", {"old": "a", "new": "b", "language": None}))
    edges.append(("empty texts", {"old": "", "new": "", "language": "python"}))
    edges.append(
        ("missing node", {"old": "x = 1\n", "new": "x = (1,\ny = 2\n", "language": "python"})
    )
    edges.append(
        ("error on a later line", {"old": "", "new": "a = 1\n\n\nb = (\n", "language": "python"})
    )
    return edges


def candidate(name: str, index: int, key: str | None = "k", **flags: bool) -> Json:
    return {
        "id": name,
        "index": index,
        "applied": flags.get("applied", True),
        "measured": flags.get("measured", True),
        "equivalence_key": key,
        "regression_passed": flags.get("regression", False),
        "reproduction_passed": flags.get("reproduction", False),
    }


def rank_edges() -> list[tuple[str, Json]]:
    rows = [
        ("no candidates", [], True),
        ("no candidates, repro invalid", [], False),
        (
            "every exclusion",
            [
                candidate("c", 2, applied=False),
                candidate("b", 1, measured=False),
                candidate("a", 0, key=""),
                candidate("d", 3, key=None),
            ],
            True,
        ),
        (
            "ties keep first appearance",
            [
                candidate("late", 5, "k2", regression=True),
                candidate("early", 1, "k1", regression=True),
                candidate("mid", 3, "k2", regression=True),
                candidate("first", 0, "k3", regression=True),
                candidate("dup", 0, "k1", regression=True),
            ],
            True,
        ),
        (
            "negative indices",
            [candidate("n", -4, "a"), candidate("m", -9, "b"), candidate("o", 7, "a")],
            False,
        ),
        (
            "reproduction rung",
            [
                candidate("r", 2, "x", regression=True, reproduction=True),
                candidate("s", 1, "y", regression=True),
                candidate("t", 0, "x", reproduction=True),
            ],
            True,
        ),
        (
            "reproduction ignored when invalid",
            [
                candidate("r", 2, "x", regression=True, reproduction=True),
                candidate("s", 1, "y", regression=True),
            ],
            False,
        ),
        (
            "falls to applied",
            [candidate("u", 1, "\U000000e9", reproduction=True), candidate("v", 0, "\x00")],
            True,
        ),
        ("one candidate", [candidate("solo", 0, "k" * 70, regression=True)], True),
        (
            "not measured but passed",
            [
                candidate("q", 0, "k", measured=False, regression=True, reproduction=True),
                candidate("p", 1, "k", regression=True),
                candidate("r", 2, "j", measured=False, regression=True),
            ],
            True,
        ),
    ]
    return [
        (name, {"candidates": candidates, "repro_valid": valid}) for name, candidates, valid in rows
    ]


def blocks_fixture() -> Fixture:
    return {"parse_blocks": section("parse_blocks", blocks_edges())}


def unidiff_fixture() -> Fixture:
    return {
        "parse_unified_diff": section("parse_unified_diff", unidiff_edges()),
        "orientation": section("orientation", orientation_edges()),
        "decimal_digits": [decimal_digits()],
    }


def decimal_digits() -> Json:
    """Return every character that a hunk header's digit pattern takes, with its int() value."""
    pattern = re.compile(r"\d")
    digits = [chr(code) for code in range(sys.maxunicode + 1) if chr(code).isdecimal()]
    taken = [chr(code) for code in range(sys.maxunicode + 1) if pattern.fullmatch(chr(code))]
    if taken != digits:
        message = "the digit pattern and str.isdecimal take different characters"
        raise SystemExit(message)
    values = "".join(str(int(digit)) for digit in digits)
    if any(int(digit) != unicodedata.decimal(digit) for digit in digits):
        message = "int() and unicodedata.decimal disagree on a digit"
        raise SystemExit(message)
    return {"name": "every decimal digit", "digits": "".join(digits), "values": values}


def apply_fixture() -> Fixture:
    return {
        "apply_edits": section("apply_edits", apply_edges()),
        "resolve_elisions": section("resolve_elisions", elision_edges()),
        "locate": section("locate", locate_edges()),
    }


def keys_fixture() -> Fixture:
    return {
        "normalized_stream": section("normalized_stream", stream_edges()),
        "file_key": section("file_key", file_key_edges()),
        "equivalence_key": section("equivalence_key", equivalence_edges()),
        "syntax_delta": section("syntax_delta", syntax_delta_edges()),
    }


def vote_fixture() -> Fixture:
    return {"rank": section("rank", rank_edges())}


def main() -> int:
    refuse_real_caches()
    builders: dict[str, Callable[[], Fixture]] = {
        "blocks": blocks_fixture,
        "unidiff": unidiff_fixture,
        "apply": apply_fixture,
        "keys": keys_fixture,
        "vote": vote_fixture,
    }
    return write_or_check("Write the patch crate's test fixtures.", FIXTURES, builders)


if __name__ == "__main__":
    raise SystemExit(main())
