"""Write the io crate's test fixtures: each case with the result that the Python package computes.

Development-time only. Run it from the repository root, in the project environment:

    uv run python scripts/io_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.
File-system cases build their tree in a temporary directory; the fixtures name its path {base}.
"""

import json
import logging
import os
import re
import sys
import tempfile
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from fixture_files import Fixture, write_or_check

from agentless_mcp.core import projectconfig
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import cachedir, fslimits
from agentless_mcp.util.errors import RepoResolutionError, SecurityRefusal, WalkBoundExceeded

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "io" / "fixtures"
CONFIG = projectconfig.CONFIG_FILENAME
CONFIG_PATH = f"root/{CONFIG}"

# A layout entry: a kind, a path below the temporary directory, and the kind's arguments.
Entry = list[Any]

DECODE = [
    b"",
    b"abc",
    b"\xc3\xa9t\xc3\xa9",
    b"\xef\xbb\xbfbom",
    b"\xf0\x9f\x98\x80",
    b"\xff",
    b"\x80",
    b"\xfe",
    b"a\xc0\x80",
    b"\xc1\xbf",
    b"\xe0\x80\x80",
    b"\xe0\x9f\xbf",
    b"\xed\xa0\x80",
    b"\xed\xbf\xbf",
    b"\xf0\x80\x80\x80",
    b"\xf4\x90\x80\x80",
    b"\xf5\x80\x80\x80",
    b"\xf8\x88\x80\x80\x80",
    b"\xe2\x82",
    b"\xe2\x82A",
    b"\xf0\x9f\x98",
    b"\xf0\x9f\x98\x80\xf0",
    b"ok\xc3",
    b"\xc3",
    b"mid \xe9 dle",
    b"0123456789\x80",
    b"\xc2\x41",
    b"\xe1\x80\x41",
    b"\xf1\x80\x80\x41",
    b"\xf0\x90\x41",
]

READ: list[list[Any]] = [
    ["missing", [], "nothing"],
    ["directory", [["dir", "d"]], "d"],
    ["empty", [["file", "e", ""]], "e"],
    ["at_cap", [["padded", "f", "x", 1_000_000]], "f"],
    ["over_cap", [["padded", "f", "x", 1_000_001]], "f"],
    ["symlink", [["padded", "f", "x", 10], ["link", "l", "f"]], "l"],
]

CONTAIN_LAYOUT: list[Entry] = [
    ["dir", "root"],
    ["dir", "root/a"],
    ["dir", "root/a/b"],
    ["file", "root/a/f.txt", "x"],
    ["dir", "outside"],
    ["file", "outside/secret", "s"],
    ["link", "root/in_dir", "a"],
    ["link", "root/in_file", "a/f.txt"],
    ["link", "root/out_dir", "../outside"],
    ["link", "root/out_file", "../outside/secret"],
    ["link", "root/abs_in", "{base}/root/a"],
    ["link", "root/abs_out", "{base}/outside"],
    ["link", "root/dangling", "nowhere"],
    ["link", "root/dangling_out", "../nowhere"],
    ["link", "root/loop", "loop"],
    ["link", "root/loop_a", "loop_b"],
    ["link", "root/loop_b", "loop_a"],
    ["link", "root/a/up", ".."],
    ["link", "root/here", "."],
    ["fifo", "root/fifo"],
]
CONTAINED = [
    "",
    ".",
    "a",
    "a/f.txt",
    "a/./f.txt",
    "a//f.txt",
    "a/f.txt/",
    "a/b/..",
    "a/b/../..",
    "..",
    "../outside/secret",
    "a/../../outside",
    "in_dir",
    "in_dir/f.txt",
    "in_dir/..",
    "in_dir/../a",
    "in_dir/b/../..",
    "out_dir",
    "out_dir/secret",
    "out_dir/../root/a",
    "out_file",
    "in_file",
    "abs_in/f.txt",
    "abs_out",
    "dangling",
    "dangling/child",
    "dangling_out",
    "missing",
    "missing/deeper/x",
    "missing/../a",
    "missing/../../outside",
    "loop",
    "loop/x",
    "loop_a",
    "a/up",
    "a/up/a/f.txt",
    "a/up/..",
    "here",
    "here/a",
    "fifo",
    "a/f.txt/x",
    "a\x00b",
    "{base}/root/a",
    "{base}/root",
    "{base}/outside",
    "{base}/root/../outside",
    "{base}/root/in_dir",
    "/",
]
STAYS_INSIDE = [
    "root/a/f.txt",
    "root/a",
    "root/in_file",
    "root/in_dir",
    "root/out_file",
    "root/out_dir",
    "root/abs_in",
    "root/dangling",
    "root/loop",
    "root/fifo",
    "root/missing",
]

WALK_LAYOUT: list[Entry] = [
    ["dir", "root"],
    ["file", "root/b.txt", "bb"],
    ["file", "root/a.txt", "a"],
    ["file", "root/Z.txt", "zz"],
    ["file", "root/.h", ""],
    ["dir", "root/sub"],
    ["file", "root/sub/c.txt", "ccc"],
    ["dir", "root/sub/deep"],
    ["file", "root/sub/deep/d.txt", "dddd"],
    ["dir", "root/sub/z"],
    ["file", "root/sub/z/x.txt", "x"],
    ["dir", "root/e"],
    ["link", "root/link_dir", "sub"],
    ["link", "root/link_file", "a.txt"],
    ["link", "root/sub/link_up", "../b.txt"],
    ["link", "root/link_out", "../outside/secret"],
    ["link", "root/dangling", "nowhere"],
    ["fifo", "root/fifo"],
    ["dir", "outside"],
    ["file", "outside/secret", "s"],
]
WALK_BOUNDS = [
    [20, 20_000, 200_000_000],
    [2, 100, 1000],
    [1, 100, 1000],
    [20, 9, 1000],
    [20, 8, 1000],
    [20, 100, 16],
    [20, 100, 15],
]
WALK_ROOTS = ["root/link_dir", "root/a.txt", "root/missing", "root/sub/deep"]

CONFIG_DOCUMENTS = [
    "{}",
    "[]",
    "null",
    "1",
    '"s"',
    "",
    "{",
    '{"map_budget": 300',
    "not json",
    "\U0000feff{}",
    '{"a": 1,}',
    '{\n  "map_budget": 300,\n\n}',
    '{"map_budget": 200}',
    '{"map_budget": 64000}',
    '{"map_budget": 199}',
    '{"map_budget": 64001}',
    '{"map_budget": 300.0}',
    '{"map_budget": 1e3}',
    '{"map_budget": true}',
    '{"map_budget": "300"}',
    '{"map_budget": null}',
    '{"map_budget": NaN}',
    '{"map_budget": 99999999999999999999}',
    '{"map_budget": -0}',
    '{"map_budget": 1, "map_budget": 300}',
    '{"max_files": 1}',
    '{"max_files": 200}',
    '{"max_files": 0}',
    '{"max_files": 201}',
    '{"granularity": "function"}',
    '{"granularity": "file"}',
    '{"granularity": "body"}',
    '{"granularity": 1}',
    '{"docstrings": true}',
    '{"docstrings": false}',
    '{"docstrings": 1}',
    '{"docstrings": "true"}',
    '{"relation_weights": true}',
    '{"relation_weights": "yes"}',
    '{"complexity_threshold": 1}',
    '{"complexity_threshold": 100}',
    '{"complexity_threshold": 0}',
    '{"complexity_threshold": 101}',
    '{"stoplist": ["foo", "bar", "foo"]}',
    '{"stoplist": []}',
    '{"stoplist": "foo"}',
    json.dumps({"stoplist": [f"n{index}" for index in range(201)] + ["late"]}),
    json.dumps({"stoplist": ["", "a b", "a/b", "x" * 65, "x" * 64, 1, None, "ok_name"]}),
    json.dumps({"stoplist": ["tab\there", "nbsp\U000000a0x", "a;b", "a\x00b", "\U00002028", "q'"]}),
    json.dumps({"stoplist": ["\U00004e2d", "\U0000200b", "x\x1fy"]}),
    '{"test_cmd": "make test"}',
    '{"test_cmd": ""}',
    '{"test_cmd": "   "}',
    '{"test_cmd": 5}',
    json.dumps({"test_cmd": "a" * 512}),
    json.dumps({"test_cmd": "a" * 513}),
    json.dumps({"test_cmd": "line\nbreak"}),
    json.dumps({"test_cmd": "cr\rhere"}),
    json.dumps({"test_cmd": "x\U00002028y"}),
    '{"erosion_gate": {"complexity_delta": "block"}}',
    json.dumps(
        {
            "erosion_gate": {
                "clone_delta": "block",
                "complexity_delta": "off",
                "contract_drift": "advisory",
            }
        }
    ),
    '{"erosion_gate": {"clone_delta": "on"}}',
    '{"erosion_gate": {"clone_delta": 1}}',
    '{"erosion_gate": {"clone_delta": ["off"]}}',
    '{"erosion_gate": {"unknown_check": "off", "other": "x"}}',
    '{"erosion_gate": {"contract_drift": "block", "zzz": "off"}}',
    '{"erosion_gate": {}}',
    '{"erosion_gate": []}',
    '{"erosion_gate": "block"}',
    '{"boundary_globs": ["src/**", "lib/*.py"]}',
    '{"boundary_globs": []}',
    '{"boundary_globs": "src"}',
    json.dumps({"boundary_globs": [f"g{index}" for index in range(65)]}),
    json.dumps({"boundary_globs": [f"g{index}" for index in range(64)]}),
    json.dumps({"boundary_globs": ["/abs", "a\\b", "", "a//b", "./a", "a/..", "ok/*", 1]}),
    json.dumps({"boundary_globs": ["x" * 257, "x" * 256, "tab\tx", "nbsp\U000000a0x"]}),
    '{"boundary_globs": ["\\u4e2d/*", "\\ud800", "a/"]}',
    '{"zeta": 1, "alpha": 2}',
    json.dumps({f"k{index}": index for index in range(8)}),
    json.dumps({f"k{index}": index for index in range(9)}),
    json.dumps({f"k{index}": index for index in range(12)}),
    json.dumps({"a\nb": 1, "it's": 2, "\U000000e9t\U000000e9": 3, "x\U00002028": 4, 'q"': 5}),
    '{"\\udcff": 1, "\\ud83d\\ude00": 2}',
    json.dumps(
        {
            "zz_unknown": 1,
            "boundary_globs": "x",
            "erosion_gate": {"clone_delta": "on", "nope": "off"},
            "map_budget": 1,
            "test_cmd": "",
            "stoplist": [""],
        }
    ),
]

CONFIG_FILES: list[list[Any]] = [
    ["absent", [["dir", "root"]]],
    ["directory", [["dir", "root"], ["dir", CONFIG_PATH]]],
    ["fifo", [["dir", "root"], ["fifo", CONFIG_PATH]]],
    ["dangling", [["dir", "root"], ["link", CONFIG_PATH, "missing.json"]]],
    ["loop", [["dir", "root"], ["link", CONFIG_PATH, CONFIG]]],
    [
        "outside",
        [
            ["dir", "root"],
            ["dir", "outside"],
            ["file", "outside/c.json", '{"map_budget": 300}'],
            ["link", CONFIG_PATH, "../outside/c.json"],
        ],
    ],
    [
        "outside_absolute",
        [
            ["dir", "root"],
            ["dir", "outside"],
            ["file", "outside/c.json", '{"map_budget": 300}'],
            ["link", CONFIG_PATH, "{base}/outside/c.json"],
        ],
    ],
    [
        "inside",
        [
            ["dir", "root"],
            ["dir", "root/sub"],
            ["file", "root/sub/c.json", '{"map_budget": 300}'],
            ["link", CONFIG_PATH, "sub/c.json"],
        ],
    ],
    ["link_to_directory", [["dir", "root"], ["dir", "root/sub"], ["link", CONFIG_PATH, "sub"]]],
    ["too_large", [["dir", "root"], ["padded", CONFIG_PATH, '{"map_budget": 300}', 65537]]],
    ["at_cap", [["dir", "root"], ["padded", CONFIG_PATH, '{"map_budget": 300}', 65536]]],
    ["not_utf8", [["dir", "root"], ["bytes", CONFIG_PATH, list(b'{"a": "\xff"}')]]],
    [
        "root_is_link",
        [
            ["dir", "real"],
            ["file", f"real/{CONFIG}", '{"max_files": 5}'],
            ["link", "root", "real"],
        ],
    ],
]

CACHE_HOME = b"/home/user"
CACHE: list[dict[str, bytes]] = [
    {"XDG_CACHE_HOME": b"/xdg", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"  /xdg \t", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": "\U000000a0/xdg\U00003000".encode(), "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"/a/./b//", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"//xdg", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"/xdg\xff", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"   ", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"rel", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"rel\xff ", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"~/x", "HOME": CACHE_HOME},
    {"XDG_CACHE_HOME": b"it's", "HOME": CACHE_HOME},
    {"HOME": CACHE_HOME},
    {"HOME": b"/home/user/"},
    {"HOME": b""},
    {"HOME": b"/"},
    {"HOME": b"//h//"},
    {"HOME": b"///h"},
    {"HOME": b"home"},
    {"HOME": b"/home/./u"},
]


@contextmanager
def tree(layout: list[Entry]) -> Iterator[Path]:
    with tempfile.TemporaryDirectory() as raw:
        base = Path(raw).resolve()
        for entry in layout:
            make(base, entry)
        yield base


def make(base: Path, entry: Entry) -> None:
    kind, relative, *rest = entry
    path = base / relative
    if kind == "dir":
        path.mkdir()
    elif kind == "fifo":
        os.mkfifo(path)
    elif kind == "link":
        path.symlink_to(rest[0].replace("{base}", str(base)))
    else:
        path.write_bytes(content(kind, rest))


def content(kind: str, rest: list[Any]) -> bytes:
    if kind == "file":
        return str(rest[0]).encode()
    if kind == "bytes":
        return bytes(rest[0])
    if kind == "padded":
        return str(rest[0]).encode().ljust(rest[1])
    message = f"unknown layout entry kind: {kind}"
    raise ValueError(message)


def placed(text: str, base: Path) -> str:
    return text.replace(str(base), "{base}")


def read_fixture() -> Fixture:
    return {
        "decode": [[list(raw), decoded(raw)] for raw in DECODE],
        "bounded": [[name, layout, path, bounded(layout, path)] for name, layout, path in READ],
    }


def decoded(raw: bytes) -> list[object]:
    try:
        return ["text", raw.decode("utf-8")]
    except UnicodeDecodeError as error:
        read = fslimits.BoundedBytes(path=Path("x"), data=raw, skipped=None)
        skipped = fslimits.decoded_strict(read).skipped
        return ["error", error.start, error.reason, skipped]


def bounded(layout: list[Entry], relative: str) -> list[object]:
    with tree(layout) as base:
        read = fslimits.read_bounded_bytes(base / relative)
    if read.data is None:
        return ["skipped", read.skipped]
    return ["bytes", len(read.data)]


def contain_fixture() -> Fixture:
    with tree(CONTAIN_LAYOUT) as base:
        root = base / "root"
        return {
            "layout": list(CONTAIN_LAYOUT),
            "contained_path": [[name, contained(root, name, base)] for name in CONTAINED],
            "file_stays_inside": [
                [name, fslimits.file_stays_inside(base / name, root.resolve())]
                for name in STAYS_INSIDE
            ],
        }


def contained(root: Path, candidate: str, base: Path) -> list[object]:
    try:
        resolved = fslimits.contained_path(root, candidate.replace("{base}", str(base)))
    except SecurityRefusal as refusal:
        return ["refused", placed(str(refusal), base)]
    return ["inside", placed(str(resolved), base)]


def walk_fixture() -> Fixture:
    with tree(WALK_LAYOUT) as base:
        root = base / "root"
        return {
            "layout": list(WALK_LAYOUT),
            "bounds": [[bounds, walked(root, bounds, base)] for bounds in WALK_BOUNDS],
            "roots": [[name, walked(base / name, WALK_BOUNDS[0], base)] for name in WALK_ROOTS],
        }


def walked(root: Path, bounds: list[int], base: Path) -> list[object]:
    max_depth, max_files, max_bytes = bounds
    try:
        files = list(
            fslimits.bounded_walk(
                root, max_depth=max_depth, max_files=max_files, max_bytes=max_bytes
            )
        )
    except (WalkBoundExceeded, RepoResolutionError) as refusal:
        return ["refused", placed(str(refusal), base)]
    return ["files", [placed(str(path), base) for path in files]]


def config_fixture() -> Fixture:
    return {
        "documents": [
            [text, loaded([["dir", "root"], ["file", CONFIG_PATH, text]])]
            for text in CONFIG_DOCUMENTS
        ],
        "files": [[name, layout, loaded(layout)] for name, layout in CONFIG_FILES],
        "serialized": [
            [text, serialized([["dir", "root"], ["file", CONFIG_PATH, text]])]
            for text in CONFIG_DOCUMENTS
        ],
    }


def serialized(layout: list[Entry]) -> str:
    # The capabilities form of the config without its warnings, which the caller renders.
    with tree(layout) as base:
        shown = projectconfig.load(base / "root").as_dict()
        del shown["warnings"]
        return placed(json.dumps(shown), base)


def loaded(layout: list[Entry]) -> dict[str, object]:
    with tree(layout) as base:
        config = projectconfig.load(base / "root")
        described = config.as_dict()
        described["path"] = None if config.path is None else placed(str(config.path), base)
        described["warnings"] = [typed_warning(warning) for warning in config.warnings]
        return described


def prompt_pattern(name: str) -> re.Pattern[str]:
    # The constant placeholders take their values; `check` and `count` capture what the call put.
    constants = {
        "config": CONFIG,
        "modes": ", ".join(projectconfig.GATE_MODES),
        "checks": ", ".join(projectconfig.GATED_CHECKS),
        "limit": str(projectconfig.MAX_BOUNDARY_GLOBS),
        "chars": str(projectconfig.MAX_BOUNDARY_GLOB_CHARS),
    }
    parts = re.split(r"\{(\w+)\}", getattr(MESSAGES, f"config_{name}"))
    pieces = []
    for index, part in enumerate(parts):
        if index % 2 == 0:
            pieces.append(re.escape(part))
        elif part in constants:
            pieces.append(re.escape(constants[part]))
        else:
            pieces.append(r"(\w+)")
    return re.compile("".join(pieces) + r"\Z")


PROMPTED = {
    name: prompt_pattern(name)
    for name in (
        "erosion_gate_not_object",
        "erosion_gate_mode",
        "erosion_gate_unknown",
        "boundary_globs_not_list",
        "boundary_globs_too_many",
        "boundary_globs_dropped",
    )
}


def typed_warning(warning: str) -> list[object]:
    # A warning from prompts/messages.json is a typed value in Rust, so it is stored as its fields.
    for name, pattern in PROMPTED.items():
        found = pattern.match(warning)
        if found:
            return [name, *(int(group) if group.isdigit() else group for group in found.groups())]
    return ["text", warning]


def cache_fixture() -> Fixture:
    return {"cache_root": [[encoded(env), *cache_root(env)] for env in CACHE]}


def encoded(env: dict[str, bytes]) -> dict[str, list[int]]:
    return {name: list(value) for name, value in env.items()}


def cache_root(env: dict[str, bytes]) -> list[object]:
    warnings: list[str] = []
    handler = Collected(warnings)
    logger = logging.getLogger(cachedir.__name__)
    logger.addHandler(handler)
    saved = {name: os.environb.get(name.encode()) for name in ("XDG_CACHE_HOME", "HOME")}
    try:
        apply_env({"XDG_CACHE_HOME": None, **env})
        cachedir._RELATIVE_CACHE_HOMES_SEEN.clear()
        root = cachedir.cache_root()
    finally:
        apply_env(saved)
        logger.removeHandler(handler)
    return [list(os.fsencode(root)), warnings[0] if warnings else None]


def apply_env(env: dict[str, bytes | None]) -> None:
    for name, value in env.items():
        if value is None:
            os.environb.pop(name.encode(), None)
        else:
            os.environb[name.encode()] = value


class Collected(logging.Handler):
    def __init__(self, sink: list[str]) -> None:
        super().__init__()
        self.sink = sink

    def emit(self, record: logging.LogRecord) -> None:
        self.sink.append(record.getMessage())


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "read": read_fixture,
    "contain": contain_fixture,
    "walk": walk_fixture,
    "config": config_fixture,
    "cache": cache_fixture,
}


if __name__ == "__main__":
    sys.exit(write_or_check(__doc__.splitlines()[0], FIXTURES, BUILDERS))
