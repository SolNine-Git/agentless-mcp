"""Write the git crate's history fixtures: the span log, churn, diff and shallow answers.

Development-time only. Run it from the repository root, with scratch cache directories:

    XDG_CACHE_HOME=<scratch> TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch> \
        .venv/bin/python scripts/history_fixtures.py

The real-repository sections build each repository from a recipe that the fixture stores, so the
Rust tests build the same objects. A byte input is a JSON string with one character per byte. With
--check, the script compares its output with the committed fixtures and writes nothing.
"""

import hashlib
import os
import random
import tempfile
from collections.abc import Sequence
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from unittest import mock

from fixture_files import (
    Fixture,
    Identity,
    hermetic_env,
    refuse_real_caches,
    run_git,
    write_or_check,
)

from agentless_mcp.application.history_service import HistoryService
from agentless_mcp.core import githistory, gitinfo
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.errors import OperationFailed
from agentless_mcp.util.tokens import Chars4Counter

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "git" / "fixtures" / "history"
NAME = "agentless-mcp tests"
EMAIL = "tests@example.invalid"
SHA = "08ec32abb9b149de9d10310c24bd12bd1fe8d874"
DATE = "2026-01-01T00:00:00+00:00"
SEED = 20261012
# A body longer than this is stored as its length and digest, so the cut log stays small.
SHOWN_BODY_CHARS = 256
ROOT_MARK = "{root}"
E_ACUTE = chr(0xE9)
REPLACEMENT = chr(0xFFFD)
# The parser's refusal with the detail alone, so the fixture holds what the git crate returns.
DETAIL_ONLY = SimpleNamespace(history_git_output_malformed="{detail}")

Step = list[Any]

REPOS: list[tuple[str, list[Step]]] = [
    (
        "span",
        [
            ["write", "a.py", "def f():\n    return 1\n"],
            ["commit", "add f", "2026-01-01T00:00:00Z"],
            ["write", "a.py", "def f():\n    return 2\n"],
            ["commit", "change f\n\nfirst line\n\n  indented\nlast\n\n", "2026-01-02T03:04:05Z"],
            ["write", "b.py", "x = 1\n"],
            ["commit", "add b", "2026-01-03T00:00:00Z"],
            ["write", "a.py", "def f():\n    return 3\n# end\n"],
            [
                "commit",
                b"bytes \xff\xfe and \xc3\xa9 \xed\xa0\x80\n\nbody \xe2\x82\n".decode("latin-1"),
                "2026-01-04T00:00:00Z",
            ],
            ["write", "a.py", "def f():\n    return 4\n# end\n"],
            ["write", "new.py", "y = 2\n"],
        ],
    ),
    (
        "big",
        [
            ["write", "a.py", "one\ntwo\nthree\n"],
            ["commit_lines", "big", 200_000, "2026-01-01T00:00:00Z"],
            ["write", "a.py", "uno\ntwo\nthree\n"],
            ["commit", "small", "2026-01-02T00:00:00Z"],
        ],
    ),
    (
        "churn",
        [
            ["write", "a.py", "1\n"],
            ["write", "old.py", "o\n"],
            ["commit", "old", "2001-01-01T00:00:00Z"],
            ["write", "a.py", "2\n"],
            ["write", "b.py", "b\n"],
            ["commit", "future one", "2090-01-01T00:00:00Z"],
            ["write", "a.py", "3\n"],
            ["write", f"d{E_ACUTE}.py", "d\n"],
            ["write", ":!x.py", "x\n"],
            ["write", "with space.py", "s\n"],
            ["commit", "future two", "2091-06-01T12:30:00Z"],
        ],
    ),
    ("shallow", [["clone_shallow", "span"]]),
    ("plain", [["mkdir"]]),
]

# The service passes the full id of HEAD's commit, so each "HEAD" below is stored as that id.
HEAD = "HEAD"
MISSING = "0" * 40
LOG_SPAN = [
    ["span", "a.py", 1, 2, 10, HEAD],
    ["span", "a.py", 1, 3, 1, HEAD],
    ["span", "a.py", 3, 3, 10, HEAD],
    ["span", "b.py", 1, 1, 10, HEAD],
    ["span", "gone.py", 1, 1, 10, HEAD],
    ["span", "a.py", 50, 60, 10, HEAD],
    ["span", "a.py", 1, 1, 10, MISSING],
    ["big", "a.py", 1, 3, 10, HEAD],
    ["plain", "a.py", 1, 1, 10, MISSING],
]
CHURN_PATHS = ["a.py", "b.py", "old.py", f"d{E_ACUTE}.py", ":!x.py", "with space.py", "missing.py"]
CHURN = [
    ["churn", CHURN_PATHS],
    ["churn", ["b.py", "b.py"]],
    ["churn", []],
    ["plain", ["a.py"]],
]
DIFFERS = [
    ["span", "b.py", HEAD],
    ["span", "a.py", HEAD],
    ["span", "new.py", HEAD],
    ["span", "gone.py", HEAD],
    ["span", "a.py", MISSING],
    ["span", "../outside.py", HEAD],
    ["plain", "a.py", MISSING],
]
SHALLOW = ["span", "shallow", "plain"]


def latin(raw: bytes) -> str:
    return raw.decode("latin-1")


def message_lines(subject: str, count: int) -> str:
    return subject + "\n\n" + "".join(f"line {number:07d}\n" for number in range(count))


def commit(repo: Path, scratch: Path, message: bytes, date: str) -> None:
    identity = Identity(NAME, EMAIL, date)
    message_file = scratch / "message"
    message_file.write_bytes(message)
    run_git(repo, identity, "add", "-A")
    run_git(repo, identity, "commit", "--quiet", "--cleanup=verbatim", "-F", str(message_file))


def build_step(base: Path, repo: Path, step: Step) -> None:
    kind = step[0]
    identity = Identity(NAME, EMAIL, DATE)
    if kind == "init":
        run_git(base, identity, "init", "--quiet", "-b", "main", str(repo))
    elif kind == "mkdir":
        repo.mkdir()
    elif kind == "clone_shallow":
        source = (base / step[1]).as_uri()
        run_git(base, identity, "clone", "--quiet", "--depth", "1", source, str(repo))
    elif kind == "write":
        target = repo / step[1]
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(step[2], encoding="utf-8")
    elif kind == "commit":
        commit(repo, base, step[1].encode("latin-1"), step[2])
    elif kind == "commit_lines":
        commit(repo, base, message_lines(step[1], step[2]).encode(), step[3])
    else:
        refusal = f"unknown recipe step {kind!r}"
        raise SystemExit(refusal)


def build(base: Path) -> None:
    for name, steps in REPOS:
        repo = base / name
        if steps[0][0] not in ("mkdir", "clone_shallow"):
            build_step(base, repo, ["init"])
        for step in steps:
            build_step(base, repo, step)


def unrooted(note: str, base: Path) -> str:
    return note.replace(str(base), ROOT_MARK)


def shown_body(body: str) -> object:
    if len(body) <= SHOWN_BODY_CHARS:
        return body
    return {"chars": len(body), "sha256": hashlib.sha256(body.encode()).hexdigest()}


def records_or_malformed(text: str, *, capped: bool) -> dict[str, object]:
    try:
        with mock.patch.object(githistory, "MESSAGES", DETAIL_ONLY):
            records = githistory.parse_log(text, capped=capped)
    except OperationFailed as error:
        return {"malformed": str(error)}
    rows = [
        [record.sha, record.authored, record.subject, shown_body(record.body)] for record in records
    ]
    return {"commits": rows}


def log_span(base: Path, request: Sequence[Any]) -> dict[str, object]:
    name, path, start, end, max_count, revision = request
    arguments = githistory.log_arguments(path, start, end, max_count=max_count, revision=revision)
    outcome = gitinfo.run_bounded(
        base / name,
        arguments,
        timeout=githistory.HISTORY_TIMEOUT_SECONDS,
        max_output_bytes=githistory.MAX_HISTORY_OUTPUT_BYTES,
    )
    if outcome.text is None:
        failure = githistory.classify_failure(outcome.note)
        return {"unanswered": unrooted(outcome.note, base), "class": failure.value}
    parsed = records_or_malformed(outcome.text, capped=outcome.truncated)
    return {**parsed, "capped": outcome.truncated}


def churn(base: Path, request: Sequence[Any]) -> object:
    name, paths = request
    facts = gitinfo.commit_churn(base / name, paths)
    if facts is None:
        return None
    return {path: [fact.commits, fact.last_commit_ts] for path, fact in facts.items()}


def answered(value: bool | None, note: str, base: Path) -> object:
    return {"unknown": unrooted(note, base)} if value is None else value


# The request with its last item, the revision, as the full commit id the service passes.
def resolved(base: Path, request: Sequence[Any]) -> list[Any]:
    *rest, revision = request
    if revision != HEAD:
        return [*rest, revision]
    head = run_git(base / rest[0], Identity(NAME, EMAIL, DATE), "rev-parse", "HEAD^{commit}")
    return [*rest, head.decode().strip()]


def real_sections() -> Fixture:
    # The service's own git answers, with its default runner.
    service = HistoryService(TreeSitterExtractor(), Chars4Counter())
    with tempfile.TemporaryDirectory() as scratch:
        base = Path(scratch).resolve()
        with mock.patch.dict(os.environ, hermetic_env(base), clear=True):
            build(base)
            spans = [resolved(base, request) for request in LOG_SPAN]
            diffs = [resolved(base, request) for request in DIFFERS]
            return {
                "log_span": [[*request, log_span(base, request)] for request in spans],
                "churn": [[*request, churn(base, request)] for request in CHURN],
                "differs": [
                    [*request, answered(*service._dirty(base / request[0], *request[1:]), base)]
                    for request in diffs
                ],
                "shallow": [
                    [name, answered(*service._shallow(base / name), base)] for name in SHALLOW
                ],
            }


def record(*fields: str) -> str:
    return "".join(f"{field}\0" for field in fields)


PARSE_LOG: list[str] = [
    "",
    "\0",
    record(SHA, DATE, "subject", "body"),
    record(SHA, DATE, "subject", "body")[:-1],
    record(SHA, DATE, "one", "") + record("b" * 40, DATE, "two", "line 1\nline 2\n"),
    record("a" * 64, DATE, "sha256", "b"),
    record(SHA.upper(), DATE, "s", "b"),
    record(SHA[:8], DATE, "s", "b"),
    record(SHA + " ", DATE, "s", "b"),
    record("a" * 41, DATE, "s", "b"),
    record("a" * 63, DATE, "s", "b"),
    record("g" * 40, DATE, "s", "b"),
    record(SHA, "", "s", "b"),
    record(SHA, DATE, "", ""),
    record(SHA, DATE, "s"),
    record(SHA, DATE, "s", "b", "extra"),
    record(SHA, DATE, "s", "b") + record(SHA, DATE),
    record(SHA, DATE, "s", "b") + record("x", DATE, "s"),
    record(SHA, DATE, "s", "b") + record(SHA, "", "s", "b"),
    record(SHA, DATE, "café", f"{REPLACEMENT} replaced\r\n\x0bnext"),
    "\0\0\0\0",
    "no nul at all",
]
CLASSIFY = [
    "git log exited 128: fatal: There is no path gone.py in the commit",
    "git cat-file exited 128: fatal: path 'a.py' does not exist in 'HEAD'",
    "git cat-file exited 128: fatal: path 'a.py' exists on disk, but not in 'abc'",
    "git log exited 128: fatal: file a.py has only 3 lines",
    "git log exited 128: fatal: file a.py has only 1 line",
    "has only",
    "lines",
    "git is not installed, so repository state is unknown",
    "git log could not be run: Too many open files",
    "git log timed out after 30.0s",
    "git log exited 128: fatal: bad revision 'nope'",
    "",
    "There is no path, and git is not installed",
    "has only 3 lines, and could not be run",
    "THERE IS NO PATH",
]


def decode_inputs() -> list[bytes]:
    singles = [bytes([byte]) for byte in range(0x80, 0x100)]
    followed = [bytes([byte]) + b"a" for byte in range(0x80, 0x100)]
    fixed = [
        b"",
        b"plain",
        f"café {chr(0x1F600)}".encode(),
        b"\xc0\x80",
        b"\xe0\x80\x80",
        b"\xf0\x80\x80\x80",
        b"\xed\xa0\x80",
        b"\xed\xbf\xbf",
        b"\xf4\x90\x80\x80",
        b"\xe2\x82",
        b"\xe2\x82a",
        b"\xf0\x9f\x98",
        b"\xf0\x9f\x98\x80\x80",
        b"\x80\x80\x80",
        b"a\xffb\xfec",
        b"\xef\xbf\xbd",
    ]
    chooser = random.Random(SEED)
    drawn = [
        bytes(chooser.getrandbits(8) for _ in range(chooser.randint(1, 12))) for _ in range(300)
    ]
    return fixed + singles + followed + drawn


def history_fixture() -> Fixture:
    return {
        "identity": [[NAME, EMAIL]],
        "repos": [[name, steps] for name, steps in REPOS],
        "parse_log": [
            [text, capped, records_or_malformed(text, capped=capped)]
            for text in PARSE_LOG
            for capped in (False, True)
        ],
        "decode": [[latin(raw), raw.decode("utf-8", errors="replace")] for raw in decode_inputs()],
        "classify": [[note, githistory.classify_failure(note).value] for note in CLASSIFY],
        "argv": argv_rows(),
        **real_sections(),
    }


def churn_argv(paths: list[str]) -> list[str]:
    seen: list[list[str]] = []

    def capture(_root: Path, arguments: Sequence[str]) -> gitinfo.GitOutcome:
        seen.append(list(arguments))
        return gitinfo.GitOutcome(None, "captured")

    with mock.patch.object(gitinfo, "_run", side_effect=capture):
        gitinfo.commit_churn(Path("/repo"), paths)
    return seen[0]


# Each call's argv after "-C <root>", as the Python builders give it.
def argv_rows() -> list[list[object]]:
    log = ["a.py", 3, 9, 11, "0" * 40]
    path, start, end, count, revision = log
    log_argv = githistory.log_arguments(path, start, end, max_count=count, revision=revision)
    return [
        ["log_span", log, log_argv],
        ["differs", ["a b.py", "a" * 64], githistory.diff_arguments("a b.py", "a" * 64)],
        ["is_shallow", [], list(githistory.SHALLOW_ARGUMENTS)],
        ["churn", CHURN_PATHS, churn_argv(CHURN_PATHS)],
    ]


BUILDERS = {"history": history_fixture}


if __name__ == "__main__":
    refuse_real_caches()
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
