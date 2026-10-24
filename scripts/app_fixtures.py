"""Write the app crate's test fixtures: each input with the result the Python services give.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory, so that no run reads or writes the user's grammar cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> uv run python scripts/app_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import dataclasses
import json
import logging
import os
import posixpath
import re
import sys
import tempfile
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from unittest import mock

from fixture_files import Fixture, Identity, hermetic_env, run_git, write_or_check

from agentless_mcp.application import capability_service, lint_service, symbol_service
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import cache, gitchanges, gitinfo
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.gitinfo import GitOutcome
from agentless_mcp.util.errors import (
    OperationFailed,
    RepoResolutionError,
    RevisionUnresolved,
    WalkBoundExceeded,
)

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "app" / "fixtures"
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"

CHECK_NAMES = list(lint_service.CHECK_NAMES)
SELECTIONS: list[list[str]] = [
    [],
    [""],
    ["complexity_delta"],
    ["clone_delta"],
    ["clone_delta", "complexity_delta"],
    ["complexity_delta", "contract_drift", "clone_delta"],
    ["arity"],
    ["complexity_delta", "arity", "shadowing"],
    ["arity", "complexity_delta", "arity"],
    ["no_such_check"],
    ["complexity_delta", "no_such_check", "other"],
    ["Complexity_delta"],
    ["complexity_delta "],
    *[[name] for name in CHECK_NAMES],
]

SCOPE_PATHS = [
    "",
    "a.py",
    "src/a.py",
    "test/a.py",
    "tests/a.py",
    "src/tests/a.py",
    "testing/a.go",
    "spec/a.rb",
    "specs/a.rb",
    "__tests__/a.ts",
    "latest/a.py",
    "tests",
    "x/tests",
    "tests/",
    "conftest.py",
    "src/conftest.py",
    "conftest",
    "os_test.go",
    "a_test",
    "contest.py",
    "shareUrl.test.ts",
    "a.spec.js",
    "a.test",
    ".test.py",
    "fixtures/a.py",
    "testdata/a.go",
    "fixtures_old/a.py",
    "src/fixtures/x/a.rs",
    "fixtures",
    "a.json",
    "a.md",
    "a",
    "dir.py/a",
    "a.PY",
    ".py",
    "a..py",
    "x.",
    "lib/a.tsx",
    "lib/a.yaml",
    "lib/a.toml",
    "lib/a.sql",
    "lib/a.c",
    "lib/a.h",
    "lib/a.hpp",
    "lib/a.sh",
    "lib/a.lua",
    "lib/a.java",
    "lib/a.kt",
    "a/b/c/d.mjs",
    "/abs/a.py",
    "//a.py",
    "a/./b.py",
    "a/../tests/b.py",
    "tést/a.py",
    "tests\\a.py",
]


def selection_row(names: list[str], form: str) -> list[object]:
    refusal = lint_service.unknown_check_refusal(names)
    if not refusal:
        if form == "path":
            refusal = lint_service.proposed_check_refusal(names)
        else:
            refusal = lint_service.revision_check_refusal(names)
    if refusal:
        return [names, form, {"refused": refusal}]
    try:
        selection = lint_service._select(names)
    except OperationFailed as refused:
        return [names, form, {"refused": str(refused)}]
    chosen = [check.name for check in lint_service.CHECKS if check.run_pairs in selection.pairs]
    return [names, form, {"chosen": chosen}]


def registry_fixture() -> Fixture:
    return {
        "checks": [[check.name, check.needs.value] for check in lint_service.CHECKS],
        "pair_checks": [list(lint_service.PAIR_CHECK_NAMES)],
        "edited_file_checks": [list(lint_service.EDITED_FILE_CHECK_NAMES)],
        "unknown_check_refusal": [
            [names, lint_service.unknown_check_refusal(names) or None] for names in SELECTIONS
        ],
        "selection": [
            selection_row(names, form) for names in SELECTIONS for form in ("path", "since")
        ],
    }


def scope_fixture() -> Fixture:
    return {
        "paths": [
            [
                path,
                symbol_service.is_test_path(path),
                symbol_service.is_fixture_path(path),
                symbol_service.in_metric_scope(path),
            ]
            for path in SCOPE_PATHS
        ],
    }


EXPAND_USER = [
    ("~", "/home/u"),
    ("~", "/home/u/"),
    ("~", "/"),
    ("~", ""),
    ("~/", "/home/u"),
    ("~/x", "/home/u"),
    ("~/x", "/home/u//"),
    ("~/x/y", ""),
    ("~//x", "/h"),
    ("x/~", "/home/u"),
    ("/abs/~", "/home/u"),
    ("", "/home/u"),
    ("~x", "/home/u"),
]


def expand_user_row(path: str, home: str) -> list[object]:
    with mock.patch.dict(os.environ, {"HOME": home}):
        return [path, home, posixpath.expanduser(path)]


def repo_fixture() -> Fixture:
    # "~x" names a user; the Rust port keeps it unchanged, which the test pins separately.
    return {
        "expand_user": [expand_user_row(path, home) for path, home in EXPAND_USER if path != "~x"],
    }


SHA = "08ec32abb9b149de9d10310c24bd12bd1fe8d874"


def outcome(case: str, subcommand: str, timeout: str) -> GitOutcome:
    """Return the Python form of the git outcome that the Rust test builds for ``case``."""
    stderr = b"fatal: bad revision 'x'\nusage: more\n"
    outcomes = {
        "printed_sha": GitOutcome(f"{SHA}\n".encode(), "", returncode=0),
        "printed_listing": GitOutcome(b"a.py\0", "", returncode=0),
        "printed_junk": GitOutcome(b"not one object name\n", "", returncode=0),
        "capped": GitOutcome(b"x" * 16, "", truncated=True, returncode=-9),
        "failed": gitinfo._bounded_outcome(subcommand, 128, b"", stderr, truncated=False),
        "not_installed": GitOutcome(None, "git is not installed, so repository state is unknown"),
        "timed_out": GitOutcome(None, f"git {subcommand} timed out after {timeout}s"),
        "not_run": GitOutcome(None, f"git {subcommand} could not be run: Permission denied"),
    }
    return outcomes[case]


def answered(value: object) -> object:
    """Return a service step's result as fixture data: a row, bytes, a skip text, or text."""
    if isinstance(value, bytes):
        return {"bytes": value.decode("latin-1")}
    if hasattr(value, "as_dict"):
        return {"row": json.dumps(value.as_dict())}
    if hasattr(value, "skipped"):
        if value.skipped is not None:
            return {"skipped": value.skipped}
        return {"bytes": value.data.decode("latin-1")}
    return {"text": value}


def review_step(case: str, step: str, subcommand: str, timeout: str) -> list[object]:
    def runner(_root: Path, _arguments: object, **_bounds: object) -> GitOutcome:
        return outcome(case, subcommand, timeout)

    review = lint_service._Review(Path("/repo"), runner)
    calls = {
        "commit": lambda: review._commit("main~2"),
        "diff": lambda: review._listing(["diff", "--raw"]),
        "ls-files": lambda: review._listing(["ls-files", "--others"]),
        "blob": lambda: review._blob(SHA, "a.py"),
    }
    try:
        return [case, step, answered(calls[step]())]
    except (OperationFailed, RevisionUnresolved) as refused:
        return [case, step, {"error": [type(refused).__name__, str(refused)]}]


STEP_CASES = {
    "commit": (
        "rev-parse",
        "5.0",
        [
            "printed_sha",
            "printed_junk",
            "capped",
            "failed",
            "not_installed",
            "timed_out",
            "not_run",
        ],
    ),
    "diff": ("diff", "30.0", ["printed_listing", "capped", "failed", "timed_out", "not_run"]),
    "ls-files": ("ls-files", "30.0", ["printed_listing", "capped", "not_installed"]),
    "blob": ("cat-file", "5.0", ["printed_listing", "capped", "failed", "timed_out"]),
}


CHANGE_SETS: list[list[tuple[str, str, str, str, str]]] = [
    [("U", "conflict.py", "conflict.py", "FILE", "FILE")],
    [("X", "odd.py", "odd.py", "FILE", "FILE")],
    [("M", "vendor", "vendor", "GITLINK", "GITLINK")],
    [("A", "lib", "lib", "ABSENT", "GITLINK")],
    [("R", "b.py", "a.py", "FILE", "FILE")],
    [("R", "a.js", "a.py", "FILE", "FILE")],
    [("D", "a.py", "a.py", "FILE", "ABSENT"), ("?", "a.py", "a.py", "ABSENT", "FILE")],
    [("T", "l.py", "l.py", "FILE", "LINK")],
    [("C", "c.py", "a.py", "ABSENT", "FILE")],
    [("M", "README.md", "README.md", "FILE", "FILE"), ("M", "z.py", "z.py", "FILE", "FILE")],
    [("M", "b.py", "b.py", "FILE", "FILE"), ("U", "a.py", "a.py", "FILE", "FILE")],
]


def change(status: str, path: str, old_path: str, old: str, new: str) -> gitchanges.Change:
    side = gitchanges.Side
    return gitchanges.Change(status, path, old_path, side[old], side[new])


def specs_row(changes: list[tuple[str, str, str, str, str]]) -> list[object]:
    rows, specs = lint_service._pair_specs([change(*entry) for entry in changes])
    shown = [[spec.path, spec.old_path, spec.new_path] for spec in specs]
    return [changes, [json.dumps(row.as_dict()) for row in rows], shown]


def outcomes_fixture() -> Fixture:
    return {
        "steps": [
            review_step(case, step, subcommand, timeout)
            for step, (subcommand, timeout, cases) in STEP_CASES.items()
            for case in cases
        ],
        "pair_specs": [specs_row(changes) for changes in CHANGE_SETS],
    }


def function(name: str, cc: int, tag: str = "") -> str:
    """Python source for one function of complexity ``cc``."""
    lines = [f"def {name}(value):"]
    for index in range(cc - 1):
        lines += [f"    if value == {index}:", f"        return {index}"]
    lines.append(f'    return "{tag}"')
    return "\n".join(lines) + "\n"


def js_function(name: str, cc: int) -> str:
    """JavaScript source for one function of complexity ``cc``."""
    branches = "".join(
        f"  if (v === {index}) {{\n    return {index};\n  }}\n" for index in range(cc - 1)
    )
    return f"function {name}(v) {{\n{branches}  return -1;\n}}\n"


def summed(name: str) -> str:
    """A five-line function over the clone_delta token floor; two names are Type-2 copies."""
    return (
        f"def {name}(items, scale=1, offset=None):\n"
        "    out = 0 * scale + len(items) - (offset or 0)\n"
        "    for item in sorted(items, key=abs, reverse=True):\n"
        "        out += item * scale - 0 + max(item, 0)\n"
        "    return round(out, 2) if out else 0\n"
    )


def repo(files: dict[str, str], name: str = "repo") -> list[list[object]]:
    """The steps that commit ``files`` as a new repository at ``name``."""
    writes = [["write", f"{name}/{path}", text] for path, text in files.items()]
    return [["mkdir", name], *writes, ["init", name], ["commit", name]]


EROSION = repo(
    {
        "app.py": function("handle", 10),
        "empty.py": "",
        "twice.py": "x = 1\n\nx = 1\n",
        "config.yaml": "key: value\n",
    }
)
BOUNDARY = repo(
    {
        ".agentless-mcp.json": json.dumps({"boundary_globs": ["adapters/**"]}),
        "adapters/api.py": "def parse(data):\n    return data['items']\n",
        "core.py": "def total(items):\n    return sum(items)\n",
    }
)
CLONES = repo(
    {
        "app.py": summed("total"),
        "tests/test_app.py": summed("tested"),
        "fixtures/sample.py": summed("sampled"),
        "README.md": "# x\n",
    }
)
OVERSIZED = ["# " + "x" * 77 + "\n", 13_000]
COMPLEXITY = ["complexity_delta"]
CONTRACT = ["contract_drift"]
CLONE = ["clone_delta"]
DEFAULT = ["complexity_delta", "contract_drift"]
GET_DEFAULT = "def parse(data):\n    return data.get('items', [])\n"
NOT_UTF8 = "x = '\xff'\n"


def proposed(
    name: str, steps: list[list[object]], path: str, after: dict[str, object], checks: list[str]
) -> dict[str, object]:
    return {
        "name": name,
        "steps": steps,
        "form": "path",
        "target": path,
        "after": after,
        "checks": checks,
    }


def since(
    name: str, steps: list[list[object]], checks: list[str], revision: str = "HEAD"
) -> dict[str, object]:
    return {"name": name, "steps": steps, "form": "since", "target": revision, "checks": checks}


def with_options(scenario: dict[str, object], **options: object) -> dict[str, object]:
    return {**scenario, **options}


PROPOSED = [
    proposed(
        "crossing_as_given", EROSION, "./app.py", {"text": function("handle", 11)}, COMPLEXITY
    ),
    proposed("new_file", EROSION, "new.py", {"text": function("fresh", 11)}, COMPLEXITY),
    proposed(
        "new_nested_file", EROSION, "pkg/sub/new.py", {"text": function("fresh", 11)}, COMPLEXITY
    ),
    proposed("empty_after", EROSION, "app.py", {"text": ""}, COMPLEXITY),
    proposed(
        "line_endings",
        [*EROSION, ["bytes", "repo/app.py", "x = 1\r\ny = 2\r\n"]],
        "app.py",
        {"text": "x = 1\r\ny = 3\r\n"},
        COMPLEXITY,
    ),
    proposed(
        "old_over_cap",
        [*EROSION, ["repeat", "repo/app.py", *OVERSIZED]],
        "app.py",
        {"text": "x = 1\n"},
        COMPLEXITY,
    ),
    proposed("new_over_cap", EROSION, "app.py", {"repeat": OVERSIZED}, COMPLEXITY),
    proposed(
        "after_not_utf8",
        [*EROSION, ["bytes", "after.py", NOT_UTF8]],
        "app.py",
        {"file": "after.py"},
        COMPLEXITY,
    ),
    proposed("after_missing", EROSION, "app.py", {"file": "gone.py"}, COMPLEXITY),
    proposed("outside", EROSION, "../outside.py", {"text": "x = 1\n"}, COMPLEXITY),
    proposed(
        "symlink_out",
        [
            *EROSION,
            ["write", "outside.py", function("handle", 2)],
            ["symlink", "repo/link.py", "<SANDBOX>/outside.py"],
        ],
        "link.py",
        {"text": function("handle", 12)},
        COMPLEXITY,
    ),
    proposed(
        "symlink_to_latin_name",
        [
            ["mkdir", "repo"],
            ["latin_name", "repo/bad\xff.py", function("handle", 1)],
            ["latin_symlink", "repo/link.py", "bad\xff.py"],
            ["init", "repo"],
        ],
        "link.py",
        {"text": function("handle", 3)},
        DEFAULT,
    ),
    proposed("readme", EROSION, "README.md", {"text": "# notes\n"}, COMPLEXITY),
    proposed("default_checks_grown", EROSION, "app.py", {"text": function("handle", 11)}, DEFAULT),
    proposed(
        "threshold_configured",
        repo(
            {
                "app.py": function("handle", 3),
                ".agentless-mcp.json": '{"complexity_threshold": 3}',
            }
        ),
        "app.py",
        {"text": function("handle", 4)},
        COMPLEXITY,
    ),
    proposed("contract_new_hit", BOUNDARY, "adapters/api.py", {"text": GET_DEFAULT}, CONTRACT),
    proposed("contract_no_rules", BOUNDARY, "adapters/run.sh", {"text": "echo hi\n"}, CONTRACT),
    proposed(
        "contract_unreadable_side",
        [*BOUNDARY, ["bytes", "after.py", NOT_UTF8]],
        "adapters/api.py",
        {"file": "after.py"},
        CONTRACT,
    ),
    proposed(
        "contract_default_checks", BOUNDARY, "adapters/api.py", {"text": GET_DEFAULT}, DEFAULT
    ),
    proposed("clone_no_tag_cache", CLONES, "lib.py", {"text": summed("again")}, CLONE),
    with_options(
        proposed("clone_fresh_cache", CLONES, "lib.py", {"text": summed("again")}, CLONE),
        cache="fresh",
    ),
    with_options(
        proposed("clone_fresh_default", CLONES, "lib.py", {"text": summed("again")}, DEFAULT),
        cache="fresh",
    ),
    with_options(
        proposed(
            "clone_out_of_scope",
            repo(
                {
                    "pkg/app.py": "def run(value):\n    return value\n",
                    "pkg/broken.py": "def broken(:\n    pass\n",
                    "tests/test_app.py": "def test_run():\n    assert True\n",
                }
            ),
            "tests/test_app.py",
            {"text": "def test_run():\n    assert 1 == 1\n"},
            CLONE,
        ),
        cache="fresh",
    ),
]

GROWN = ["write", "repo/app.py", function("handle", 11)]
COPIED = ["write", "repo/lib.py", summed("again")]

REVISION = [
    since("modified_crossing", [*EROSION, GROWN], COMPLEXITY),
    since(
        "renamed",
        [
            *EROSION,
            ["git", "repo", "mv", "app.py", "moved.py"],
            ["write", "repo/moved.py", function("handle", 11)],
        ],
        COMPLEXITY,
    ),
    since("deleted", [*EROSION, ["remove", "repo/app.py"]], COMPLEXITY),
    since(
        "moved_out_of_deleted",
        [
            *repo({"app.py": function("big", 15)}),
            ["remove", "repo/app.py"],
            ["write", "repo/other.py", function("big", 15)],
        ],
        COMPLEXITY,
    ),
    since(
        "staged_and_untracked",
        [
            *EROSION,
            ["write", "repo/staged.py", function("staged", 11)],
            ["git", "repo", "add", "staged.py"],
            ["write", "repo/loose.py", function("loose", 11)],
        ],
        COMPLEXITY,
    ),
    since(
        "committed_and_uncommitted",
        [*EROSION, GROWN, ["commit", "repo"], ["write", "repo/loose.py", function("loose", 11)]],
        COMPLEXITY,
        revision="HEAD~1",
    ),
    since(
        "binary",
        [*repo({"blob.py": "x = 1\n"}), ["bytes", "repo/blob.py", "x = 1\0\n"]],
        COMPLEXITY,
    ),
    since(
        "submodule",
        [*EROSION, ["gitlink", "repo", "vendor/lib"], ["mkdir", "repo/vendor/lib"]],
        COMPLEXITY,
    ),
    since(
        "untracked_nested_repository",
        [
            *EROSION,
            ["mkdir", "repo/nested"],
            ["init", "repo/nested"],
            ["write", "repo/nested/inner.py", "x = 1\n"],
            ["commit", "repo/nested"],
        ],
        COMPLEXITY,
    ),
    since(
        "removed_from_index",
        [*EROSION, ["git", "repo", "rm", "-q", "--cached", "app.py"], GROWN],
        COMPLEXITY,
    ),
    since("outside_tier1", [*EROSION, ["write", "repo/config.yaml", "key: other\n"]], COMPLEXITY),
    since("no_change", EROSION, COMPLEXITY),
    since(
        "symlink_replaced_file",
        [
            *repo({"app.py": function("big", 15), "keep.py": "x = 1\n"}),
            ["remove", "repo/app.py"],
            ["symlink", "repo/app.py", "keep.py"],
            ["symlink", "repo/alias.py", "keep.py"],
        ],
        COMPLEXITY,
    ),
    since("no_such_branch", EROSION, COMPLEXITY, revision="no-such-branch"),
    since("tree_revision", EROSION, COMPLEXITY, revision="HEAD^{tree}"),
    since("option_revision", EROSION, COMPLEXITY, revision="--output=x"),
    since(
        "contract_moved_between_boundary_files",
        [
            *BOUNDARY,
            ["write", "repo/adapters/api.py", GET_DEFAULT],
            ["commit", "repo"],
            ["write", "repo/adapters/api.py", "VERSION = 2\n"],
            ["write", "repo/adapters/parsing.py", GET_DEFAULT],
        ],
        CONTRACT,
    ),
    since(
        "cross_language_rename",
        [
            *EROSION,
            ["git", "repo", "mv", "app.py", "app.js"],
            ["write", "repo/app.js", js_function("handle", 11)],
        ],
        COMPLEXITY,
    ),
    with_options(
        since(
            "root_below_git_top",
            [
                *repo({"pkg/app.py": function("handle", 10)}),
                ["write", "repo/pkg/app.py", function("handle", 11)],
                ["write", "repo/pkg/loose.py", function("loose", 11)],
            ],
            COMPLEXITY,
        ),
        root="repo/pkg",
    ),
    since(
        "old_side_line_endings",
        [
            *EROSION,
            ["bytes", "repo/crlf.py", "x = 1\r\ny = 2\r\n"],
            ["git", "repo", "-c", "core.autocrlf=false", "add", "crlf.py"],
            ["commit", "repo"],
            ["bytes", "repo/crlf.py", "x = 1\r\ny = 3\r\n"],
        ],
        DEFAULT,
    ),
    since("default_checks", [*EROSION, GROWN], DEFAULT),
    since("clone_copy", [*CLONES, COPIED], CLONE),
    with_options(since("clone_budget_spent", [*CLONES, COPIED], CLONE), budget="zero"),
    since(
        "clone_unreadable_file",
        [*CLONES, COPIED, ["bytes", "repo/latin.py", "y = 1\nx = '\xff'\n"]],
        CLONE,
    ),
    since("clone_default_checks", [*CLONES, COPIED], DEFAULT),
    since(
        "clone_out_of_scope", [*CLONES, ["write", "repo/fixtures/sample.py", summed("more")]], CLONE
    ),
]

IDENTITY = Identity("app tests", "app@example.invalid", "2026-02-01T00:00:00Z")


def _write(_sandbox: Path, target: Path, rest: list[object]) -> None:
    _bytes(_sandbox, target, [str(rest[0]).encode().decode("latin-1")])


def _bytes(_sandbox: Path, target: Path, rest: list[object]) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(str(rest[0]).encode("latin-1"))


def _latin_name(sandbox: Path, target: Path, rest: list[object]) -> None:
    # The file name is the latin-1 bytes of its path, so a character above U+007F is no UTF-8.
    relative = str(target.relative_to(sandbox)).encode("latin-1")
    named = Path(os.fsdecode(os.fsencode(sandbox) + b"/" + relative))
    named.parent.mkdir(parents=True, exist_ok=True)
    named.write_bytes(str(rest[0]).encode())


def _mtime(_sandbox: Path, target: Path, rest: list[object]) -> None:
    ns = int(datetime.fromisoformat(str(rest[0])).timestamp()) * 10**9
    os.utime(target, ns=(ns, ns))


def _latin_symlink(_sandbox: Path, target: Path, rest: list[object]) -> None:
    target.symlink_to(os.fsdecode(str(rest[0]).encode("latin-1")))


def _repeat(_sandbox: Path, target: Path, rest: list[object]) -> None:
    _write(_sandbox, target, [str(rest[0]) * int(str(rest[1]))])


def _symlink(sandbox: Path, target: Path, rest: list[object]) -> None:
    target.symlink_to(str(rest[0]).replace("<SANDBOX>", str(sandbox)))


def _remove(_sandbox: Path, target: Path, _rest: list[object]) -> None:
    target.unlink()


def _mkdir(_sandbox: Path, target: Path, _rest: list[object]) -> None:
    target.mkdir(parents=True, exist_ok=True)


def _init(_sandbox: Path, target: Path, _rest: list[object]) -> None:
    run_git(target, IDENTITY, "init", "--quiet", "-b", "main")


def _commit(_sandbox: Path, target: Path, _rest: list[object]) -> None:
    run_git(target, IDENTITY, "add", "-A")
    run_git(target, IDENTITY, "commit", "--quiet", "--allow-empty", "-m", "fixture")


def _gitlink(_sandbox: Path, target: Path, rest: list[object]) -> None:
    head = run_git(target, IDENTITY, "rev-parse", "HEAD").decode().strip()
    run_git(target, IDENTITY, "update-index", "--add", "--cacheinfo", f"160000,{head},{rest[0]}")


def _git(_sandbox: Path, target: Path, rest: list[object]) -> None:
    run_git(target, IDENTITY, *(str(argument) for argument in rest))


STEPS: dict[str, Callable[[Path, Path, list[object]], None]] = {
    "write": _write,
    "bytes": _bytes,
    "latin_name": _latin_name,
    "latin_symlink": _latin_symlink,
    "mtime": _mtime,
    "repeat": _repeat,
    "symlink": _symlink,
    "remove": _remove,
    "mkdir": _mkdir,
    "init": _init,
    "commit": _commit,
    "gitlink": _gitlink,
    "git": _git,
}


def apply(sandbox: Path, step: list[object]) -> None:
    """Run one scenario step in ``sandbox``. The Rust test runs the same steps."""
    kind, place, *rest = step
    STEPS[str(kind)](sandbox, sandbox / str(place), rest)


class _FreshCache(cache.OnDemandSource):
    """A source that the service treats as a fresh tag cache: its blocks equal a re-measure."""

    @property
    def cached(self) -> bool:
        return True


def after_path(sandbox: Path, after: dict[str, object]) -> Path:
    if "file" in after:
        return sandbox / str(after["file"])
    path = sandbox / "after-text"
    if "repeat" in after:
        line, count = after["repeat"]
        path.write_text(str(line) * int(count), encoding="utf-8")
    else:
        path.write_text(str(after["text"]), encoding="utf-8")
    return path


def judged(sandbox: Path, scenario: dict[str, object]) -> dict[str, object]:
    extractor = TreeSitterExtractor()
    ctx = resolve_repo(sandbox / str(scenario.get("root", "repo")), None)
    fresh = scenario.get("cache") == "fresh"
    source = _FreshCache(extractor) if fresh else cache.OnDemandSource(extractor)
    ctx = dataclasses.replace(ctx, symbols=source)
    service = lint_service.LintService(extractor)
    checks = tuple(scenario["checks"])
    try:
        if scenario["form"] == "path":
            after = after_path(sandbox, scenario["after"])
            view = service.lint_proposed(ctx, str(scenario["target"]), after, checks)
        else:
            view = service.lint_revision(ctx, str(scenario["target"]), checks)
    except (OperationFailed, RevisionUnresolved) as refused:
        message = str(refused).replace(str(sandbox), "<SANDBOX>")
        return {"error": [type(refused).__name__, message]}
    report = json.dumps(view.as_dict()).replace(str(sandbox), "<SANDBOX>")
    # The approved non-UTF-8 rule: the Rust build names a byte that is not UTF-8 with U+FFFD.
    return {"report": re.sub(r"\\udc[89a-f][0-9a-f]", r"\\ufffd", report)}


@contextmanager
def isolated(sandbox: Path) -> Iterator[None]:
    """Point HOME, the caches and the git config at ``sandbox`` for one scenario."""
    home = sandbox / "home"
    home.mkdir()
    with mock.patch.dict(os.environ, hermetic_env(home)):
        yield


def scenario_row(scenario: dict[str, object]) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="app-fixture-") as made:
        sandbox = Path(made).resolve()
        with isolated(sandbox):
            for step in scenario["steps"]:
                apply(sandbox, step)
            spent = scenario.get("budget") == "zero"
            budget = (lambda _left: 0.0) if spent else lint_service.deadline.walk_budget
            with mock.patch.object(lint_service.deadline, "walk_budget", budget):
                expected = judged(sandbox, scenario)
    return {**scenario, "expected": expected}


def lint_fixture() -> Fixture:
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    os.environ["AGENTLESS_MCP_NO_DOWNLOAD"] = "1"
    os.environ["AGENTLESS_MCP_NO_AUTO_WARM"] = "1"
    return {
        "proposed": [scenario_row(scenario) for scenario in PROPOSED],
        "revision": [scenario_row(scenario) for scenario in REVISION],
    }


FULL_CONFIG = {
    "map_budget": 4000,
    "max_files": 25,
    "granularity": "file",
    "docstrings": True,
    "stoplist": ["zeta", "alpha"],
    "test_cmd": "pytest -q",
    "relation_weights": True,
    "complexity_threshold": 12,
    "erosion_gate": {"clone_delta": "off", "complexity_delta": "block"},
    "boundary_globs": ["adapters/**", "api/*.py"],
}
WARNED_CONFIG = {
    "map_budget": 5,
    "granularity": "tree",
    "test_cmd": "",
    "erosion_gate": {"complexity_delta": "loud", "lint": "block"},
    "boundary_globs": ["/abs/**", "ok/**"],
    "zeta_key": 1,
    "alpha_key": 2,
}
CAPABILITY_CASES: list[dict[str, object]] = [
    {"name": "plain", "config": None, "no_cache": False},
    {"name": "full_config", "config": FULL_CONFIG, "no_cache": False},
    {"name": "warned_config", "config": WARNED_CONFIG, "no_cache": False},
    {
        "name": "gate_not_object",
        "config": {"erosion_gate": [], "boundary_globs": 3},
        "no_cache": False,
    },
    {"name": "bypassed", "config": None, "no_cache": True},
    {"name": "corrupt_cache", "config": None, "no_cache": False, "corrupt": True},
]


def capability_row(case: dict[str, object]) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="app-fixture-") as made:
        sandbox = Path(made).resolve()
        with isolated(sandbox):
            files = {"app.py": "x = 1\n"}
            if case["config"] is not None:
                files[".agentless-mcp.json"] = json.dumps(case["config"])
            for step in repo(files):
                apply(sandbox, step)
            root = sandbox / "repo"
            database = cache.cache_path(root)
            if case.get("corrupt"):
                database.parent.mkdir(parents=True)
                database.write_text("not a database")
            extractor = TreeSitterExtractor()
            ctx = resolve_repo(root, None)
            source = cache.open_source(
                ctx.root, extractor, tree_oid=ctx.tree_oid, no_cache=bool(case["no_cache"])
            )
            ctx = dataclasses.replace(ctx, symbols=source)
            document = capability_service.build_capability_report(ctx, extractor).as_dict()
            # Both fields depend on the environment, and the Rust build reports its own values.
            document["version"] = "<VERSION>"
            document["grammar_cache"] = "<GRAMMAR_CACHE>"
            text = json.dumps(document).replace(database.parent.name, "<KEY>")
            source.close()
    return {**case, "report": text.replace(str(sandbox), "<SANDBOX>")}


def capabilities_fixture() -> Fixture:
    return {"reports": [capability_row(case) for case in CAPABILITY_CASES]}


DEEP = "/".join(f"d{level}" for level in range(1, 22))
UNCOMMITTED = [
    ["mkdir", "repo"],
    ["init", "repo"],
    ["write", "repo/a.py", "def f(x):\n    return x\n"],
    ["latin_name", "repo/bad\xff.py", "z = 1\n"],
]
# Fixed mtimes, so the digest of the walked files is the same in every run.
PINNED_TREE = [
    ["mkdir", "repo"],
    ["write", "repo/a.py", "x = 1\n"],
    ["write", "repo/pkg/b.py", "y = 22\n"],
    ["mtime", "repo/a.py", "2000-01-01T00:00:00Z"],
    ["mtime", "repo/pkg/b.py", "2001-02-03T04:05:06Z"],
]
# A repository with no commit has no tree id, so a usable cache takes its generation from a walk.
# A pinned case records the generation too.
GENERATION_CASES: list[dict[str, object]] = [
    {"name": "walk_warned", "steps": UNCOMMITTED, "cache_home": None, "pinned": False},
    {
        "name": "walk_refused_after_warnings",
        "steps": [*UNCOMMITTED, ["write", f"repo/{DEEP}/x.txt", "x\n"]],
        "cache_home": "rel",
        "pinned": False,
    },
    {"name": "digest_outside_git", "steps": PINNED_TREE, "cache_home": None, "pinned": True},
]


class _Warnings(logging.Handler):
    """The warnings the Python build logs, as its command line prints them on stderr."""

    def __init__(self) -> None:
        super().__init__(level=logging.WARNING)
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


def generation_row(case: dict[str, object]) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="app-fixture-") as made:
        sandbox = Path(made).resolve()
        with isolated(sandbox):
            steps = case["steps"]
            assert isinstance(steps, list)
            for step in steps:
                apply(sandbox, step)
            if case["cache_home"] is not None:
                os.environ["XDG_CACHE_HOME"] = str(case["cache_home"])
            root = sandbox / "repo"
            warnings = _Warnings()
            logging.getLogger().addHandler(warnings)
            generation, refused = None, None
            try:
                # The cache open reads the cache root first, then the generation.
                cache.cache_path(root)
                generation = cache.repo_generation(root, None)
            except (RepoResolutionError, WalkBoundExceeded) as error:
                refused = str(error).replace(str(sandbox), "<SANDBOX>")
            finally:
                logging.getLogger().removeHandler(warnings)
    lines = [line.replace(str(sandbox), "<SANDBOX>") for line in warnings.lines]
    pinned = generation if case["pinned"] else None
    return {**case, "warnings": lines, "refused": refused, "generation": pinned}


def generation_fixture() -> Fixture:
    return {
        "cases": [generation_row(case) for case in GENERATION_CASES],
        "walk_budget_seconds": [lint_service.deadline.WALK_BUDGET_SECONDS],
    }


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "capabilities": capabilities_fixture,
    "generation": generation_fixture,
    "lint": lint_fixture,
    "outcomes": outcomes_fixture,
    "registry": registry_fixture,
    "repo": repo_fixture,
    "scope": scope_fixture,
}


if __name__ == "__main__":
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
