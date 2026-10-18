"""Write the server argument fixtures: each agentless-mcp-server argv with the reference answer.

Development-time only. Run it from the repository root with the project environment:

    uv run python scripts/server_args_fixtures.py

Each run calls the reference parser in process, with the working directory and HOME inside one
temporary sandbox, which the fixtures write as <SANDBOX>. The version text is the Rust workspace
version, so a wrapped --version line is exact. A run whose answer the Rust build gives by a ledger
deviation holds the adjusted reference answer, its deviation label, and the reference's own outcome.
With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import argparse
import contextlib
import ipaddress
import os
import sys
import tempfile
from collections.abc import Callable, Iterator, Sequence
from pathlib import Path
from types import ModuleType
from typing import Any
from unittest import mock

import tomllib
from fixture_files import render, write_or_check_files
from gate_report_fixtures import ascii_int, captured

from agentless_mcp.adapters.mcp import cliargs

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "cli" / "fixtures" / "server_args"
PLACEHOLDER = "<SANDBOX>"
# A host name resolves through /etc/hosts, whose order differs between machines.
RESOLVED = "<LOOPBACK>"
# Without a terminal the reference parser wraps at 80 columns, as the Rust parser does.
NO_TERMINAL_COLUMNS = "80"
LAYOUT: list[dict[str, str]] = [
    {"dir": "repo-a"},
    {"dir": "repo-b"},
    {"dir": "home/proj"},
    {"dir": "work/rel"},
    {"dir": "roots-dir"},
    {"file": "file.txt", "hex": b"text\n".hex()},
    {"link": "link-a", "to": "repo-a"},
    {"file": "roots-ok.txt", "hex": b"# repos\n\n<SANDBOX>/repo-a\n  rel-b  \n~/proj\n".hex()},
    {"file": "roots-bom.txt", "hex": b"\xef\xbb\xbf<SANDBOX>/repo-b\n".hex()},
    {"file": "roots-bad.bin", "hex": b"/srv/repo\n\xff\n".hex()},
    {"file": "work/roots-rel.txt", "hex": b"rel\n".hex()},
]
# DV2: the two help texts that named FastMCP and described the removed surfaces.
DV2_HELP = {
    "--transport": (
        "how the server talks to its client: stdio (the default) for a client that launches "
        "this process, http for one long-lived server several clients share over the "
        "streamable-http transport"
    ),
    "--surface": (
        "which tool surface to publish: v2, the six consolidated intent-shaped tools, is the "
        "only one. v1 and both were removed in 0.11.0 and are refused at startup"
    ),
}
# DV3: the surfaces that 0.11.0 removed are refused after the reference's own checks.
DV3_TEXT = (
    "--surface {surface} is no longer served: 0.11.0 removed the v1 tools. Drop --surface, or "
    "pass --surface v2. The v1-to-v2 mapping table is in the agent guide: agentless-mcp guide "
    "--section the-two-surfaces."
)
PORTS = ("0", "-1", "1", "65535", "65536", "8766", "9" * 25, "x", "0x10", " 8 ", "+5", "1_0", "007")
HOSTS = (
    "127.0.0.1",
    "127.0.0.2",
    "127.1",
    "::1",
    "::ffff:127.0.0.1",
    "0127.0.0.1",
    str(ipaddress.IPv4Address(0)),
    "::",
    "8.8.8.8",
    "",
    "localhost",
    "LOCALHOST",
)
LIMITS = ("0", "-0.0", "-1", "1e-7", "1e6", "1234567", "inf", "nan", "-inf", "x", "1_000.5", " 2 ")
GRACES = ("-1.5", "-0.0", "0", "inf", "nan", "1e400")
COUNTS = ("0", "65", "1", "64", "-3", "9" * 25, "x")

Json = dict[str, Any]
Types = dict[object, object]


def workspace_version() -> str:
    """Return the version of the Rust workspace, which the Rust --version prints."""
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    return str(manifest["workspace"]["package"]["version"])


@contextlib.contextmanager
def sandbox() -> Iterator[Path]:
    """Build the layout in a new temporary directory and yield its resolved path."""
    with tempfile.TemporaryDirectory() as raw:
        path = Path(raw).resolve()
        for entry in LAYOUT:
            if "dir" in entry:
                (path / entry["dir"]).mkdir(parents=True)
            elif "link" in entry:
                (path / entry["link"]).symlink_to(path / entry["to"])
            else:
                data = bytes.fromhex(entry["hex"]).replace(PLACEHOLDER.encode(), bytes(path))
                (path / entry["file"]).write_bytes(data)
        yield path


@contextlib.contextmanager
def environment(path: Path, columns: str | None) -> Iterator[None]:
    """Run inside the sandbox's work directory, with its home and one terminal width."""
    saved = {name: os.environ.get(name) for name in ("HOME", "COLUMNS")}
    before = Path.cwd()
    os.environ["HOME"] = str(path / "home")
    os.environ["COLUMNS"] = NO_TERMINAL_COLUMNS if columns is None else columns
    os.chdir(path / "work")
    try:
        yield
    finally:
        os.chdir(before)
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def ascii_float(text: str) -> float:
    """The deviation's real number rule (DV5): float() for an ASCII text."""
    if not text.isascii():
        raise ValueError(text)
    return float(text)


def literal_root(raw: str) -> Path:
    """The deviation's --root rule (DV7): a leading ~name is a relative path, never a user."""
    if raw.startswith("~") and not raw.startswith("~/") and raw != "~":
        path = Path(raw).resolve()
        if not path.is_dir():
            message = f"--root {raw} is not a directory: {path}"
            raise argparse.ArgumentTypeError(message)
        return path
    return cliargs.root_dir(raw)


ascii_float.__name__ = "float"
literal_root.__name__ = "root_dir"


def patched_argparse(types: Types) -> ModuleType:
    """Return argparse for the reference module, its parser with DV2 and a deviation's types."""

    class Patched(argparse.ArgumentParser):
        def add_argument(self, *args: Any, **kwargs: Any) -> argparse.Action:
            if args and args[0] in DV2_HELP:
                kwargs["help"] = DV2_HELP[args[0]]
            if kwargs.get("type") in types:
                kwargs["type"] = types[kwargs["type"]]
            return super().add_argument(*args, **kwargs)

    # A copy of the module: argparse itself names ArgumentParser in its own super() calls.
    module = ModuleType(argparse.__name__)
    module.__dict__.update(vars(argparse))
    module.ArgumentParser = Patched
    return module


def checked_surface(original: Callable[..., None]) -> Callable[..., None]:
    """Return the last start-up check with the DV3 refusal after it."""

    def check(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
        original(parser, args)
        if args.surface != cliargs.SURFACE_V2:
            parser.error(DV3_TEXT.format(surface=args.surface))

    return check


@contextlib.contextmanager
def rust_rules(types: Types) -> Iterator[None]:
    """Patch the reference parser with the Rust version, DV2, DV3 and the run's deviation types."""
    last_check = checked_surface(cliargs._check_call_limits)
    with (
        mock.patch.object(cliargs, "argparse", patched_argparse(types)),
        mock.patch.object(cliargs, "server_version", return_value=workspace_version()),
        mock.patch.object(cliargs, "_check_call_limits", last_check),
    ):
        yield


def shown(value: object, path: Path) -> object:
    """Return ``value`` with the sandbox path written as the placeholder, in every text."""
    if isinstance(value, str):
        return value.replace(str(path), PLACEHOLDER)
    if isinstance(value, list):
        return [shown(item, path) for item in value]
    if isinstance(value, dict):
        return {key: shown(item, path) for key, item in value.items()}
    return value


def host_shown(host: str | None, literal: str | None) -> str | None:
    """Return the bound literal, or the placeholder when a host name chose it."""
    if literal is None or host is None:
        return literal
    try:
        ipaddress.ip_address(host)
    except ValueError:
        return RESOLVED
    return literal


def parsed(args: argparse.Namespace) -> Json:
    """Return the values of a parse that the Rust ServerArgs holds."""
    http = args.transport == cliargs.TRANSPORT_HTTP
    host, port = cliargs.http_binding(args) if http else (None, None)
    return {
        "roots": [str(root) for root in args.root],
        "roots_from": [str(file.path) for file in args.roots_from],
        "no_auto_index": args.no_auto_index,
        "no_auto_restart": args.no_auto_restart,
        "transport": args.transport,
        "host": host_shown(args.host, host),
        "port": port,
        "max_concurrency": args.max_concurrency,
        "call_limit": repr(args.call_limit),
        "recache_grace": repr(args.recache_grace),
    }


def parse_in_sandbox(
    argv: Sequence[str], columns: str | None, rules: contextlib.AbstractContextManager[None]
) -> Json:
    """Run the parser once in a fresh sandbox; return the run with sandbox paths shown."""
    with sandbox() as path:
        real = [arg.replace(PLACEHOLDER, str(path)) for arg in argv]
        with environment(path, columns), rules:
            out, err, value, crash = captured(lambda: cliargs.parse_args(real))
        run: Json = {"argv": list(argv), "columns": columns, "out": out, "err": err}
        namespace = value if isinstance(value, argparse.Namespace) else None
        run["code"] = 0 if namespace is not None else value
        run["parsed"] = parsed(namespace) if namespace is not None else None
        if crash is not None:
            run["crash"] = crash
        result = shown(run, path)
    assert isinstance(result, dict)
    return result


def run(argv: Sequence[str], columns: str | None = None) -> Json:
    """Return one run under the Rust rules; refuse a run on which the reference crashes."""
    result = parse_in_sandbox(argv, columns, rust_rules({}))
    if "crash" in result:
        message = f"refused: the reference parser crashed on {list(argv)}: {result['crash']}"
        raise SystemExit(message)
    return result


def deviation(label: str, argv: Sequence[str], types: Types) -> Json:
    """Return a run whose Rust answer follows a deviation, beside the reference's outcome."""
    result = parse_in_sandbox(argv, None, rust_rules(types))
    result["deviation"] = label
    result["reference"] = reference_outcome(argv)
    return result


def reference_outcome(argv: Sequence[str]) -> object:
    """Return the reference's own outcome: its values, its refusal, or the error it raises."""
    try:
        own = parse_in_sandbox(argv, None, contextlib.nullcontext())
    # DV7: the reference's expanduser raises this for a user the system does not know.
    except RuntimeError as error:
        return f"{type(error).__name__}: {error}"
    return own.get("crash") or own["parsed"] or own["err"]


def help_runs() -> list[Json]:
    runs = [run(["--help"], width) for width in (None, "20", "40", "200")]
    runs += [run(["--version"], width) for width in (None, "20", "12")]
    runs.append(run(["-h"]))
    return runs


def transport_runs() -> list[Json]:
    http = ["--transport", "http"]
    argvs: list[list[str]] = [
        [],
        http,
        ["--host", "127.0.0.1"],
        ["--port", "8000"],
        ["--host", "::1", "--port", "1"],
        ["--port", "x"],
        [*http, "--allow-client-roots"],
        ["--allow-client-roots"],
        ["--transport", "tcp"],
        ["--transport=http", "--port=8766"],
    ]
    argvs += [[*http, "--port", port] for port in PORTS]
    argvs += [[*http, "--host", host] for host in HOSTS]
    return [run(argv) for argv in argvs]


def limit_runs() -> list[Json]:
    argvs: list[list[str]] = [["--max-concurrency", count] for count in COUNTS]
    argvs += [["--call-limit", limit] for limit in LIMITS]
    argvs += [["--recache-grace", grace] for grace in GRACES]
    argvs.append(["--max-concurrency", "0", "--call-limit", "0"])
    argvs.append(["--call-limit", "0", "--recache-grace", "-1"])
    return [run(argv) for argv in argvs]


def flag_runs() -> list[Json]:
    argvs: list[list[str]] = [
        ["--no-auto-warm", "--no-auto-index", "--no-auto-restart"],
        ["--surface", "v2"],
        ["--surface", "v3"],
        ["--surface", "v1"],
        ["--surface", "both", "--transport", "http"],
        ["--surface", "v1", "--port", "0", "--transport", "http"],
        ["--bogus"],
        ["--root"],
        ["--no-auto"],
        ["--no-auto-r"],
        ["--roo", f"{PLACEHOLDER}/repo-a"],
        ["--root /a --root /b"],
        ["--root '/a b"],
        ["-x", "-y"],
        ["--", "x"],
    ]
    return [run(argv) for argv in argvs]


def root_runs() -> list[Json]:
    repo_a, repo_b = f"{PLACEHOLDER}/repo-a", f"{PLACEHOLDER}/repo-b"
    missing, roots_ok = f"{PLACEHOLDER}/missing", f"{PLACEHOLDER}/roots-ok.txt"
    argvs: list[list[str]] = [
        ["--root", repo_a],
        ["--root", "rel"],
        ["--root", "~"],
        ["--root", "~/proj"],
        ["--root", f"{PLACEHOLDER}/link-a"],
        ["--root", f"{PLACEHOLDER}/file.txt"],
        ["--root", missing],
        ["--root", "missing"],
        ["--root", repo_a, "--root", repo_b, "--root", repo_a],
        [f"--root={repo_a}"],
        ["--roots-from", roots_ok],
        ["--roots-from", f"{PLACEHOLDER}/roots-bom.txt", "--roots-from", roots_ok],
        ["--roots-from", f"{PLACEHOLDER}/roots-bad.bin"],
        ["--roots-from", f"{PLACEHOLDER}/roots-dir"],
        ["--roots-from", f"{PLACEHOLDER}/missing.txt"],
        ["--roots-from", "roots-rel.txt"],
        ["--root", missing, "--port", "x"],
        ["--port", "x", "--root", missing],
        ["--roots-from", f"{PLACEHOLDER}/missing.txt", "--root", missing],
        ["--root", missing, "--help"],
        ["--help", "--root", missing],
        ["--version", "--port", "x"],
        ["--port", "x", "--version"],
        ["--root", repo_a, "--roots-from", roots_ok, "--transport", "http", "--port", "8766"],
    ]
    return [run(argv) for argv in argvs]


def arabic(digits: str) -> str:
    """Return ``digits`` written with ARABIC-INDIC digits, which int() and float() accept."""
    return "".join(chr(0x0660 + int(digit)) for digit in digits)


def deviation_runs() -> list[Json]:
    http = ["--transport", "http"]
    return [
        deviation("DV5", [*http, "--port", arabic("8000")], {int: ascii_int}),
        deviation("DV5", ["--max-concurrency", arabic("2")], {int: ascii_int}),
        deviation("DV5", ["--call-limit", arabic("1")], {float: ascii_float}),
        deviation("DV7", ["--root", "~nosuchuser-agentless/x"], {cliargs.root_dir: literal_root}),
    ]


def build_files() -> dict[Path, bytes]:
    """Return the fixture file of the server's command line."""
    runs = help_runs() + transport_runs() + limit_runs() + flag_runs() + root_runs()
    runs += deviation_runs()
    layout: list[object] = list(LAYOUT)
    return {FIXTURES / "parser.json": render({"layout": layout, "runs": list(runs)})}


def main() -> int:
    """Write the fixtures, or compare them with --check, and return the exit status."""
    return write_or_check_files(__doc__.splitlines()[0], FIXTURES, build_files)


if __name__ == "__main__":
    sys.exit(main())
