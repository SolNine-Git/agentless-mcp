"""Write a crate's fixture files from their builders, or compare them with --check.

The fixture generators import this module. With write_or_check, each file holds one JSON object of
named sections, and each section is a list of cases, one case per line. With write_or_check_files,
the generator renders each file itself.
"""

import argparse
import json
import sys
from collections.abc import Callable
from pathlib import Path

PYTHON = (3, 13)

Fixture = dict[str, list[object]]


def write_or_check(
    description: str, directory: Path, builders: dict[str, Callable[[], Fixture]]
) -> int:
    """Write directory/<name>.json for each builder, or with --check compare and write nothing."""
    return write_or_check_files(
        description,
        directory,
        lambda: {directory / f"{name}.json": render(build()) for name, build in builders.items()},
    )


def write_or_check_files(
    description: str, directory: Path, build: Callable[[], dict[Path, bytes]]
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
        stale = [path for path, text in contents.items() if not matches(path, text)]
        for path in stale:
            print(f"stale: {path}", file=sys.stderr)
        return 1 if stale else 0
    directory.mkdir(parents=True, exist_ok=True)
    for path, text in contents.items():
        path.write_bytes(text)
        print(f"wrote {path}")
    return 0


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
