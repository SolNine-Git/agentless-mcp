"""Write the git crate's test fixtures: each parser input with the result the Python functions give.

Development-time only. Run it from the repository root:

    uv run python scripts/git_fixtures.py

A byte input is a JSON string with one character per byte (U+0000 to U+00FF). With --check, the
script compares its output with the committed fixtures and writes nothing.
"""

import logging
from collections.abc import Callable
from pathlib import Path
from unittest import mock

from fixture_files import Fixture, write_or_check

from agentless_mcp.core import gitchanges, gitinfo, treewalk
from agentless_mcp.core.gitchanges import Change
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import OperationFailed, WalkBoundExceeded

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "git" / "fixtures"
WALK_ROOT = Path("/repo")
SHA = "08ec32abb9b149de9d10310c24bd12bd1fe8d874"
LS = "\N{LINE SEPARATOR}"
PS = "\N{PARAGRAPH SEPARATOR}"
IDEOGRAPHIC = "\N{IDEOGRAPHIC SPACE}"


def record(old_mode: str, new_mode: str, status: str, *paths: bytes) -> bytes:
    """Return one `git diff --raw -z` record, each field ended by a NUL."""
    meta = f":{old_mode} {new_mode} b1bfde2 0000000 {status}".encode()
    return b"".join(field + b"\0" for field in (meta, *paths))


MODES = ["000000", "100644", "100755", "120000", "160000"]
BAD_MODES = ["040000", "", "1006440", "0123456789abcdef", "it's", 'say"x"', "10\t44\n", "\x7f\x1b"]

PARSE_CHANGES = [
    b"",
    record("100644", "100644", "M", b"a.py"),
    record("000000", "100755", "A", b"run.py"),
    record("100644", "000000", "D", b"gone.py"),
    record("100644", "100644", "R086", b"old.py", b"new.py"),
    record("100644", "100644", "C075", b"src.py", b"copy.py"),
    record("100644", "120000", "T", b"link.py"),
    record("000000", "160000", "A", b"vendor/lib"),
    record("000000", "000000", "U", b"conflict.py"),
    record("100644", "100644", "R100", b"a.py", b"b.py") + record("100644", "100644", "M", b"c.py"),
    b":100644 100644 b1 00 M\0name\xff.py\0",
    b"a.py\0",
    b":100644 100644 b1 00\0a.py\0",
    b":100644 100644 b1 00 R100\0a.py\0",
    b":100644 040000 b1 00 M\0a.py\0",
    *(record(old, new, "M", b"m.py") for old in MODES for new in MODES),
    *(record(bad, "100644", "M", b"m.py") for bad in BAD_MODES),
    *(record("100644", bad, "M", b"m.py") for bad in BAD_MODES),
    record("040000", "100644", "C050", b"from.py", b"to.py"),
    record("100644", "040000", "C050", b"from.py", b"to.py"),
    b":10\xff44 100644 a b M\0m.py\0",
    b":100644 100644 a b M\0a.py",
    b":100644 100644 a b M\0a.py\0\0",
    b":100644 100644 a b M\0\0",
    b":100644 100644 a b M\0",
    b":100644 100644 a b M",
    b":100644 100644 a b R100\0",
    b":100644 100644 a b C\0only.py\0",
    b":100644 100644 a b \0a.py\0",
    b":100644 100644 a b M x\0a.py\0",
    b":100644 100644 a b\0a.py\0",
    b": 100644 a b M\0a.py\0",
    b":\0a.py\0",
    b":::::\0a.py\0",
    b"\0",
    b"\0\0",
    b"100644 100644 a b M\0a.py\0",
    b" :100644 100644 a b M\0a.py\0",
    record("100644", "100644", "X", b"x.py"),
    record("100644", "100644", "B", b"b.py"),
    record("100644", "100644", "\xe9tat", b"e.py"),
    b":100644 100644 a b \xff\0f.py\0",
    record("100644", "100644", "M", b"with space.py"),
    record("100644", "100644", "M", b"line\nbreak.py"),
    record("100644", "100644", "M", "\xfcber/na\xefve.py".encode()),
    record("100644", "100644", "M", b"surrogate\xed\xa0\x80.py"),
    record("100644", "100644", "M", b"overlong\xc0\x80.py"),
    record("100644", "100644", "M", b"partial\xe2\x82"),
    record("100644", "100644", "M", b"lead\xf0\x80\x80.py"),
    record("100644", "100644", "M", b"high\xf4\x90\x80\x80.py"),
    record("100644", "100644", "R100", b"old\xff.py", b"new\xfe.py"),
]

PARSE_UNTRACKED = [
    b"",
    b"new.py\0nested/\0",
    b"new.py",
    b"a//\0",
    b"/\0",
    b"dir/sub/\0",
    b"\0",
    b"\0\0",
    b"bad\xff.py\0",
    b"bad\xff/\0",
    "\xfcber.py\0".encode(),
    b"one.py\0two.py\0one.py\0",
    b"trailing\0\0",
]

PARSE_COMMIT = [
    f"{SHA}\n".encode(),
    ("a" * 64 + "\n").encode(),
    SHA.encode(),
    b"",
    SHA[:8].encode(),
    f"{SHA}\n{SHA}".encode(),
    b"not a sha",
    SHA.upper().encode(),
    f"  {SHA}\t\r\n".encode(),
    f"\x0b{SHA}\x0c".encode(),
    f"\xa0{SHA}{IDEOGRAPHIC}".encode(),
    f"\x1c{SHA}\x1f".encode(),
    f"{SHA}\0".encode(),
    (SHA + "a").encode(),
    ("a" * 63).encode(),
    ("a" * 65).encode(),
    ("a" * 104).encode(),
    SHA[:39].encode() + b"\xff",
    ("f" * 4096).encode(),
]

LOOKS_BINARY = [
    b"",
    b"\0",
    b"bin\0ary",
    b"x" * gitchanges.BINARY_PROBE_BYTES + b"\0",
    b"x" * (gitchanges.BINARY_PROBE_BYTES - 1) + b"\0",
    "def f():\n    return '\xe9'\n".encode(),
    b"\xff\xfe\x00",
]

RECEIPT = [
    b"",
    b"\n",
    b"72f5a2ff\n",
    b"  abc  \n",
    b"\x0babc\x0c",
    b"\x1cabc\x1f",
    b"\xc2\xa0abc\xc2\xa0",
    b"\xa0abc",
    b"abc\xff\n",
    b"\r\n\tabc def\r\n",
    b" M a.py\n?? new.py\n",
    b" M a.py\n\n\n?? b\n",
    b"a\r\nb\rc\x0bd\x0ce",
    f"a\x1cb\x1dc\x1ed\x85e{LS}f{PS}g".encode(),
    b"   \n\t\n",
    f"x\n{IDEOGRAPHIC}\ny\n".encode(),
    b"bad\xff\nok\n",
    b"\x85\n",
]

EXIT_STDERR = [
    b"",
    b"\n",
    b"fatal: bad revision 'x'\n",
    b"\n\nfatal: first\nsecond\n",
    b"  lead\n",
    b"\xff\xfe err\n",
    b"error\r\nnext",
    b"a\x1cb",
    LS.encode(),
    f"{LS}text".encode(),
    b"\x0bvertical\x0b",
    b"E" * 300,
]
EXIT_CODES = [1, 128, -9]

DECODED_PATHS = [
    (b"app.py\0bad\xff.py\0pkg/mod.py\0", 20_000),
    ("\xfcber/na\xefve.py\0".encode(), 20_000),
    (b"".join(f"f{index}.py\0".encode() for index in range(6)), 5),
    (b"".join(f"f{index}.py\0".encode() for index in range(5)), 5),
    (b"a.py\0a.py\0a.py\0b.py\0", 2),
    (b"", 20_000),
    (b"\0\0\0", 20_000),
    (b"a.py\0b.py", 20_000),
    (b"bad\xff.py\0bad\xff.py\0ok.py\0", 20_000),
    (b"s\xed\xa0\x80.py\0o\xc0\x80.py\0p\xe2\x82\0", 20_000),
    (b"nested/\0x.py\0", 20_000),
    (b"a.py\0a.py\0", 1),
    (b"a.py\0b.py\0", 1),
    (b"x\xff\0y\xff\0z\xff\0", 1),
    (b"a.py\0", 0),
]


def latin(raw: bytes) -> str:
    """Return the bytes as a string with one character per byte."""
    return raw.decode("latin-1")


def detail(error: OperationFailed) -> str:
    """Return the detail that the malformed-output message wraps."""
    prefix, suffix = MESSAGES.lint_revision_output_malformed.format(detail="\0").split("\0")
    message = str(error)
    if not (message.startswith(prefix) and message.endswith(suffix)):
        refusal = f"not a malformed-output message: {message!r}"
        raise SystemExit(refusal)
    return message[len(prefix) : len(message) - len(suffix)]


def change_row(change: Change) -> list[str]:
    """Return one change as [status, path, old_path, old side, new side]."""
    return [change.status, change.path, change.old_path, change.old.value, change.new.value]


def changes_or_malformed(raw: bytes) -> dict[str, object]:
    """Return the changes parse_changes reads, or the detail of its refusal."""
    try:
        return {"changes": [change_row(change) for change in gitchanges.parse_changes(raw)]}
    except OperationFailed as error:
        return {"malformed": detail(error)}


def commit_or_malformed(raw: bytes) -> dict[str, object]:
    """Return what parse_commit makes of the lossily decoded stdout, as the review passes it."""
    try:
        return {"commit": gitchanges.parse_commit(raw.decode("utf-8", errors="replace"))}
    except OperationFailed as error:
        return {"malformed": detail(error)}


def receipt_row(raw: bytes, *, truncated: bool) -> list[object]:
    """Return [input, truncated, text, note, dirty count] for what gitinfo._run makes of ``raw``."""
    bounded = gitinfo.GitOutcome(raw, "", truncated=truncated, returncode=-9 if truncated else 0)
    with mock.patch.object(gitinfo, "run_bounded", return_value=bounded):
        outcome = gitinfo._run(Path(), ["status", "--porcelain"])
    dirty = gitinfo._parse_dirty(outcome)
    return [latin(raw), truncated, outcome.text, outcome.note, dirty.count]


class _Counts(logging.Handler):
    def __init__(self) -> None:
        super().__init__(logging.WARNING)
        self.counts: list[object] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.counts.append(record.args[0] if isinstance(record.args, tuple) else None)


def decoded_row(raw: bytes, max_files: int) -> list[object]:
    """Return [input, max_files, outcome] for one walk listing, the warning counts included."""
    handler = _Counts()
    logger = logging.getLogger(treewalk.__name__)
    logger.addHandler(handler)
    try:
        paths = treewalk._decoded_paths(raw, WALK_ROOT, max_files=max_files)
        outcome: dict[str, object] = {"paths": paths, "undecodable": handler.counts}
    except WalkBoundExceeded as error:
        outcome = {"refused": str(error)}
    finally:
        logger.removeHandler(handler)
    return [latin(raw), max_files, outcome]


def changes_fixture() -> Fixture:
    return {
        "parse_changes": [[latin(raw), changes_or_malformed(raw)] for raw in PARSE_CHANGES],
        "parse_untracked": [
            [latin(raw), [change_row(change) for change in gitchanges.parse_untracked(raw)]]
            for raw in PARSE_UNTRACKED
        ],
        "parse_commit": [[latin(raw), commit_or_malformed(raw)] for raw in PARSE_COMMIT],
        "looks_binary": [[latin(raw), gitchanges.looks_binary(raw)] for raw in LOOKS_BINARY],
    }


def receipt_fixture() -> Fixture:
    return {
        "receipt": [
            receipt_row(raw, truncated=truncated) for raw in RECEIPT for truncated in (False, True)
        ],
        "exit_note": [
            [
                latin(stderr),
                code,
                gitinfo._bounded_outcome("rev-parse", code, b"", stderr, truncated=False).note,
            ]
            for stderr in EXIT_STDERR
            for code in EXIT_CODES
        ],
    }


def walk_fixture() -> Fixture:
    return {"decoded_paths": [decoded_row(raw, max_files) for raw, max_files in DECODED_PATHS]}


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "changes": changes_fixture,
    "receipt": receipt_fixture,
    "walk": walk_fixture,
}


if __name__ == "__main__":
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
