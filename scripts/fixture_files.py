"""Write a crate's fixture files from their builders, or compare them with --check.

The fixture generators import this module. With write_or_check, each file holds one JSON object of
named sections, and each section is a list of cases, one case per line. With write_or_check_files,
the generator renders each file itself. The module also holds the scratch environment for the
generators that run git or read a cache.
"""

import argparse
import json
import os
import pwd
import subprocess
import sys
from collections.abc import Callable, Collection, Mapping
from dataclasses import dataclass
from pathlib import Path

from agentless_mcp.core import grammars

PYTHON = (3, 13)
GIT_SECONDS = 60
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"
# tiktoken reads the cl100k_base file from here, so that no generator reads the network.
TIKTOKEN_CACHE = "TIKTOKEN_CACHE_DIR"
NO_GIT_CONFIG = {"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}

Fixture = dict[str, list[object]]


@dataclass(frozen=True)
class Identity:
    """The author and committer that a fixture's git commits record, and their clock."""

    name: str
    email: str
    date: str

    def pinned(self) -> dict[str, str]:
        """Return the six GIT_ names that pin this identity on a commit."""
        return {
            "GIT_AUTHOR_NAME": self.name,
            "GIT_AUTHOR_EMAIL": self.email,
            "GIT_AUTHOR_DATE": self.date,
            "GIT_COMMITTER_NAME": self.name,
            "GIT_COMMITTER_EMAIL": self.email,
            "GIT_COMMITTER_DATE": self.date,
        }


def write_or_check(
    description: str,
    directory: Path,
    builders: Mapping[str, Callable[[], Fixture]],
    shared: Collection[Path] = (),
) -> int:
    """Write directory/<name>.json for each builder, or with --check compare and write nothing."""
    return write_or_check_files(
        description,
        directory,
        lambda: {directory / f"{name}.json": render(build()) for name, build in builders.items()},
        shared,
    )


def write_or_check_files(
    description: str,
    directory: Path,
    build: Callable[[], dict[Path, bytes]],
    shared: Collection[Path] = (),
) -> int:
    """Write each file that ``build`` returns, or with --check compare and write nothing."""
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument(
        "--check", action="store_true", help="compare with the committed fixtures, write nothing"
    )
    args = parser.parse_args()
    if sys.version_info[:2] != PYTHON:
        found = sys.version.split()[0]
        print(f"refused: the fixtures need Python 3.13; found {found}", file=sys.stderr)
        return 2

    contents = build()
    if args.check:
        lines = differences(contents, shared)
        for line in lines:
            print(line, file=sys.stderr)
        return 1 if lines else 0
    directory.mkdir(parents=True, exist_ok=True)
    # A generator can also write to the fixtures of another crate.
    for path, text in contents.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text)
        print(f"wrote {path}")
    return 0


def differences(contents: Mapping[Path, bytes], shared: Collection[Path] = ()) -> list[str]:
    """Return a "stale" line for each file that differs and an "extra" line for each orphan."""
    stale = [f"stale: {path}" for path, text in contents.items() if not matches(path, text)]
    return stale + [f"extra: {path}" for path in extra_files(contents, shared)]


def extra_files(produced: Collection[Path], shared: Collection[Path]) -> list[Path]:
    """Return the files beside ``produced``, with a produced suffix, that nothing here produces.

    A file at or below an entry of ``shared`` belongs to another generator and is never returned.
    """
    places = {(path.parent, path.suffix) for path in produced}
    present = {path for parent, suffix in places for path in parent.glob(f"*{suffix}")}
    return sorted(
        path
        for path in present - set(produced)
        if path.is_file() and not any(path.is_relative_to(entry) for entry in shared)
    )


def matches(path: Path, text: bytes) -> bool:
    """Return True when ``path`` holds exactly ``text``."""
    return path.is_file() and path.read_bytes() == text


def render(sections: Fixture) -> bytes:
    """Return the fixture text of ``sections``: ASCII JSON with one case per line."""
    # One case per line, so a changed case shows as one changed line in a diff.
    blocks = [
        f" {json.dumps(name)}: [\n" + ",\n".join(f"  {json.dumps(case)}" for case in cases) + "\n ]"
        for name, cases in sections.items()
    ]
    return ("{\n" + ",\n".join(blocks) + "\n}\n").encode("ascii")


def hermetic_env(home: Path) -> dict[str, str]:
    """Return an environment with only PATH from the caller, HOME in ``home`` and no git config."""
    return {
        "PATH": os.environ["PATH"],
        "HOME": str(home),
        "XDG_CACHE_HOME": str(home / ".cache"),
        **NO_GIT_CONFIG,
    }


def git_env(identity: Identity, base: Mapping[str, str]) -> dict[str, str]:
    """Return ``base`` without its GIT_ names, with no git config file and ``identity`` pinned."""
    # A calling git hook sets GIT_DIR, GIT_INDEX_FILE and their relatives, which move git to another
    # repository. Git can add to that family, so every GIT_ name goes.
    kept = {name: value for name, value in base.items() if not name.startswith("GIT_")}
    return {**kept, **NO_GIT_CONFIG, **identity.pinned()}


def run_git(
    directory: Path, identity: Identity, *arguments: str, env: Mapping[str, str] | None = None
) -> bytes:
    """Run git in ``directory`` with ``identity`` pinned, and return its stdout or raise."""
    command = ["git", "-C", str(directory), *arguments]
    environment = git_env(identity, os.environ if env is None else env)
    done = subprocess.run(
        command, env=environment, check=True, capture_output=True, timeout=GIT_SECONDS
    )
    return done.stdout


def refuse_real_caches(environ: Mapping[str, str] = os.environ) -> None:
    """Exit with a refusal when the grammar override, XDG_CACHE_HOME or HOME reaches ~/.cache."""
    # The passwd entry, not $HOME: a scratch HOME must not move the tree that the guard protects.
    real_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    real = (real_home / ".cache").resolve()
    places = {
        GRAMMAR_OVERRIDE: environ.get(GRAMMAR_OVERRIDE),
        "XDG_CACHE_HOME": environ.get("XDG_CACHE_HOME"),
        "HOME": str(Path(environ.get("HOME") or real_home) / ".cache"),
    }
    for name, place in places.items():
        if place and Path(place).resolve().is_relative_to(real):
            sys.exit(f"refused: {name} puts a cache in {real}. Set it to a scratch directory.")


def refuse_unready_environment(environ: Mapping[str, str] = os.environ) -> None:
    """Exit with a refusal unless the caches are scratch copies and every grammar is warm."""
    refuse_real_caches(environ)
    if not environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    if not environ.get(TIKTOKEN_CACHE):
        sys.exit(f"refused: set {TIKTOKEN_CACHE} to a scratch copy of the cl100k_base file")
    # The Rust build links every grammar, so a cold grammar here would pin a different answer.
    cold = set(grammars.ALL_LANGUAGES) - grammars.warmed_languages()
    if cold:
        sys.exit(f"refused: {GRAMMAR_OVERRIDE} holds no library for {', '.join(sorted(cold))}")
