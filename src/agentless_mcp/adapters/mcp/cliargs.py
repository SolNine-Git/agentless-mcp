"""The server's command line, importable without the ``mcp`` extra.

Everything ``agentless-mcp-server`` needs to parse and validate its argv lives
here, deliberately outside :mod:`agentless_mcp.adapters.mcp.server`, which
imports ``fastmcp`` at module scope and therefore cannot load on a bare
install. Splitting the parser out is what lets the entry point answer
``--help`` and ``--version``, and refuse a malformed argv, before the gated
import runs -- a bare install used to exit 2 on the missing extra for the one
invocation a user makes to find out whether the thing works (#47).

Nothing here may import ``fastmcp``, ``mcp`` or ``pydantic``, directly or
transitively; ``tests/unit/test_bootstrap.py`` proves it in a subprocess.
"""

import argparse
import ipaddress
import shlex
import socket
import sys
import threading
from collections.abc import Sequence
from dataclasses import dataclass, field
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as distribution_version
from pathlib import Path
from typing import Literal

from agentless_mcp.application.repo_context import resolved_allowlist
from agentless_mcp.core import cache, grammars, selfrestart
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import SecurityRefusal

# The distribution name from pyproject, which the installed metadata is the only
# source of truth for. It equals the server's advertised name today by
# coincidence, not by rule.
DISTRIBUTION_NAME = "agentless-mcp"

# Announced in the initialize handshake when the metadata is absent, which
# happens only in a source tree that was never installed. Not an empty string:
# a blank version reads as a server that answered rather than one that could
# not look itself up.
UNKNOWN_VERSION = "0+unknown"


def server_version() -> str:
    """The version the initialize handshake advertises for this server."""
    try:
        return distribution_version(DISTRIBUTION_NAME)
    except PackageNotFoundError:
        sys.stderr.write(
            f"agentless-mcp-server: no installed metadata for {DISTRIBUTION_NAME}, so the "
            f"initialize handshake will report version {UNKNOWN_VERSION}. This is a source "
            "tree that was never installed; install it to report a real version.\n"
        )
        return UNKNOWN_VERSION


# The transports this server will start. stdio is the default because a client
# that spawns the server as a child is the shape every registered client uses,
# and because it is the only one that needs no port, no binding decision and no
# second process to outlive the client.
# Annotated as literals because FastMCP types its own transport parameter as a
# Literal: a bare str here would type-check at the constant and fail at the call.
TRANSPORT_STDIO: Literal["stdio"] = "stdio"
TRANSPORT_HTTP: Literal["http"] = "http"
TRANSPORTS = (TRANSPORT_STDIO, TRANSPORT_HTTP)

# Where --transport http listens when the operator names no address. Loopback,
# and enforced rather than merely defaulted -- see _check_transport. The port is
# FastMCP's own default: deferring to the framework's documented number keeps
# this package from carrying one deployment's port allocation, and every
# deployment that cares passes --port anyway.
DEFAULT_HTTP_HOST = "127.0.0.1"
DEFAULT_HTTP_PORT = 8000

# The ports an operator may name. Zero is excluded deliberately: it binds an
# ephemeral port this process never reports, so the operator cannot register a
# client against it.
MIN_HTTP_PORT = 1
MAX_HTTP_PORT = 65535

# The tool surfaces this server can publish. v2 -- five intent-shaped tools --
# is the default; v1 keeps the original eleven for un-migrated operators, and
# both publishes the union for a transition window (find_referencing_symbols
# and capabilities are shared by the two surfaces, so the union is fourteen
# names, not sixteen). The flag is server-level: one process publishes one
# surface, whatever repositories it serves.
SURFACE_V1: Literal["v1"] = "v1"
SURFACE_V2: Literal["v2"] = "v2"
SURFACE_BOTH: Literal["both"] = "both"
SURFACES = (SURFACE_V1, SURFACE_V2, SURFACE_BOTH)
Surface = Literal["v1", "v2", "both"]


def _root_lines(text: str) -> list[Path]:
    """The paths one roots file's text lists, blank and #-comment lines skipped."""
    # Whole-line comments only: a repository path may itself contain '#'.
    return [
        Path(stripped)
        for line in text.splitlines()
        if (stripped := line.strip()) and not stripped.startswith("#")
    ]


@dataclass
class RootsFile:
    """One ``--roots-from`` file and the roots it held when last read.

    The file is the operator-editable half of the allowlist: ``current()``
    re-reads it when its modification time changes, so an appended line enrols
    a repository on the next call and a removed line revokes one, without a
    restart. A file that stops being readable after startup is refused loudly
    on every call that needs it, never served from the stale copy: silence
    here would leave the operator editing a file the server had quietly
    stopped watching. Startup validation still happens in ``roots_file``, so
    a bad path fails the process before it ever answers a call.
    """

    path: Path
    roots: list[Path]
    stat_key: tuple[int, int]
    lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def current(self) -> list[Path]:
        """This file's roots as of now, re-read if the file changed on disk.

        Change is keyed on (mtime_ns, size) rather than mtime alone: a
        rewrite landing within the filesystem's timestamp granularity almost
        always changes the byte count too, and both fields come from the one
        stat the freshness check already pays for.

        Held under a lock because the HTTP transport reaches this from
        concurrent request threads and the two fields are one fact: the roots
        and the stat key they were read at. Publishing them separately lets a
        second caller observe new roots against an old key, which costs a
        redundant re-read now and would cost correctness the moment anything
        else keyed on the pair. The same reasoning already guards the
        background index's registry.
        """
        with self.lock:
            try:
                stat_result = self.path.stat()
            except OSError as exc:
                message = MESSAGES.roots_file_unreadable.format(file=self.path, error=exc)
                raise SecurityRefusal(message) from exc
            key = (stat_result.st_mtime_ns, stat_result.st_size)
            if key == self.stat_key:
                return self.roots
            try:
                text = self.path.read_text(encoding="utf-8-sig")
            except (OSError, UnicodeDecodeError) as exc:
                message = MESSAGES.roots_file_unreadable.format(file=self.path, error=exc)
                raise SecurityRefusal(message) from exc
            self.roots = resolved_allowlist(_root_lines(text))
            self.stat_key = key
            return self.roots


def roots_file(raw: str) -> RootsFile:
    """Read one ``--roots-from`` file, or refuse it by name at startup.

    Foreign input, parsed here and nowhere else. ``utf-8-sig`` rather than
    ``utf-8`` because a BOM would otherwise survive into the first entry, where
    it turns an absolute path into a relative one and yields a root that
    silently never matches. ``OSError`` and ``UnicodeDecodeError`` are converted
    explicitly: ``UnicodeDecodeError`` is a ``ValueError``, which argparse
    swallows in favour of a generic "invalid value" message that loses the
    reason. The stat comes before the read so a write landing between the two
    leaves a recorded mtime older than the content, which costs one redundant
    re-read on the next call rather than a stale answer.
    """
    path = Path(raw)
    try:
        stat_result = path.stat()
        text = path.read_text(encoding="utf-8-sig")
    except OSError as exc:
        message = f"cannot read roots file {raw}: {exc}"
        raise argparse.ArgumentTypeError(message) from exc
    except UnicodeDecodeError as exc:
        message = f"roots file {raw} is not valid UTF-8: {exc}"
        raise argparse.ArgumentTypeError(message) from exc

    return RootsFile(
        path=path,
        roots=resolved_allowlist(_root_lines(text)),
        stat_key=(stat_result.st_mtime_ns, stat_result.st_size),
    )


def root_dir(raw: str) -> Path:
    """Resolve one ``--root`` flag to an existing directory, or refuse it here.

    A mistyped ``--root`` used to start the server and fail on every tool call
    with "not a directory", which under an MCP client surfaces per call rather
    than at spawn -- a wiring error found at first use instead of at startup.
    ``--roots-from`` already stats and reads its file at parse time; this is
    the same standard for the flag beside it.

    The *contents* of a roots file stay unchecked on purpose: that file is
    re-read live, so a line naming a repository nobody has cloned yet is a
    defensible thing to write. A flag is fixed for the process lifetime and
    has no such second chance.
    """
    path = Path(raw).expanduser()
    try:
        resolved = path.resolve()
    except OSError as exc:
        message = f"--root {raw}: {exc}"
        raise argparse.ArgumentTypeError(message) from exc
    if not resolved.is_dir():
        message = f"--root {raw} is not a directory: {resolved}"
        raise argparse.ArgumentTypeError(message)
    return resolved


def _looks_unsplit(element: str) -> bool:
    """Is this one argv element an option flag glued to its own value(s)?

    The server's parser declares no positional arguments, so an element whose
    first token lexes as an option and which lexes into more than one token
    cannot be anything but a shell string that was never word-split. Unbalanced
    quotes make the element unlexable, which is not evidence either way, and
    raising here would replace argparse's exit 2 with a traceback.
    """
    try:
        tokens = shlex.split(element)
    except ValueError:
        return False
    return len(tokens) > 1 and tokens[0].startswith("-")


def _report_argv(argv: Sequence[str]) -> None:
    """Describe the argv argparse just rejected, on stderr, for a captured log."""
    sys.stderr.write(f"agentless-mcp-server: received argv: {list(argv)!r}\n")
    for index, element in enumerate(argv):
        if not _looks_unsplit(element):
            continue
        sys.stderr.write(
            f"agentless-mcp-server: argv element {index} looks like an unsplit "
            "shell string; check quoting in the client registration. Pass each "
            'token as its own argv element (e.g. "--root", "/a", "--root", '
            '"/b"), or use --roots-from FILE.\n'
        )


def _loopback_literal(host: str) -> str | None:
    """The IP literal to bind for ``host``, or None when it is not loopback-only.

    Resolution rather than a string comparison: ``localhost``, ``127.0.0.1``,
    ``::1`` and a hosts-file alias are all the same decision, and a name that
    resolves to a routable address is that decision's opposite however
    local it looks. A name that resolves to nothing is not loopback either --
    the caller reports it as a refusal rather than binding something else.

    One lookup, and the literal it returns is the one that gets bound.
    Checking with one ``getaddrinfo`` and binding after another left a window
    in which the records could change between the two, so a name that passed
    the check could still put a routable address on the socket.
    """
    try:
        candidates = socket.getaddrinfo(host, None, proto=socket.IPPROTO_TCP)
    except socket.gaierror:
        return None
    addresses = [str(info[4][0]) for info in candidates]
    if not addresses:
        return None
    if not all(ipaddress.ip_address(address).is_loopback for address in addresses):
        return None
    return addresses[0]


def _check_transport(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    """Refuse a transport/binding combination at startup rather than at first use.

    Two refusals, both about a flag that would otherwise be silently ignored
    or silently obeyed:

    ``--host``/``--port`` under stdio have no meaning -- there is no socket --
    and an operator who passes them believes they got a listener. Saying so at
    startup costs one line; discovering it costs a debugging session against a
    port nothing is on.

    A non-loopback bind is refused outright because this server authenticates
    nobody. Its entire access-control story is the ``--root`` allowlist, which
    decides *which repositories* are readable and says nothing about *who* may
    read them. On a routable address that is not an allowlist at all: it is
    unauthenticated read access to the source of every enrolled repository.
    Anything wider than loopback therefore needs an authenticating proxy in
    front, which is a deployment decision this process cannot make for itself.

    The port is held to the same standard as the host beside it: ``--port
    99999`` used to fail inside ``server.run`` as an opaque bind error, and
    ``--port 0`` used to bind an ephemeral port the operator cannot predict
    and was never told about.

    The verified host literal is stashed on the namespace here so that
    :func:`http_binding` binds the address this check resolved rather than
    resolving the name a second time.
    """
    # Always present, so a reader never has to know which branch ran.
    args.host_literal = None
    if args.transport == TRANSPORT_STDIO:
        passed = (("--host", args.host), ("--port", args.port))
        given = [flag for flag, value in passed if value is not None]
        if given:
            parser.error(
                f"{' and '.join(given)} apply to --transport {TRANSPORT_HTTP} and the "
                f"transport is {TRANSPORT_STDIO}, which has no socket to bind. Pass "
                f"--transport {TRANSPORT_HTTP} to serve over HTTP, or drop the flag."
            )
        return
    if args.allow_client_roots:
        parser.error(
            f"--allow-client-roots cannot be combined with --transport {TRANSPORT_HTTP}. "
            "That flag lets the connected client's advertised roots authorise "
            "repositories, which is safe under stdio because the client is the process "
            "that spawned this server. Over HTTP the client is whatever reaches the "
            "port, and loopback is not per-user isolated, so any local process could "
            "name its own root and read anything this server's user can read -- the "
            "--root allowlist would stop deciding what is servable. Drop "
            "--allow-client-roots and enrol the repositories with --root or "
            "--roots-from."
        )
    port = args.port if args.port is not None else DEFAULT_HTTP_PORT
    if not MIN_HTTP_PORT <= port <= MAX_HTTP_PORT:
        parser.error(
            f"--port {port} is outside {MIN_HTTP_PORT}-{MAX_HTTP_PORT}. Port 0 binds an "
            "ephemeral port this process never reports, and anything above the range "
            "fails inside the transport as an opaque bind error. Name a port a client "
            "can be registered against."
        )
    host = args.host if args.host is not None else DEFAULT_HTTP_HOST
    literal = _loopback_literal(host)
    if literal is None:
        parser.error(
            f"--host {host!r} is not a loopback address. This server authenticates no "
            "one: the --root allowlist decides which repositories are readable, not who "
            "may read them, so binding it where the network can reach it publishes every "
            f"enrolled repository. Bind {DEFAULT_HTTP_HOST} and put an authenticating "
            "proxy in front if you need it off-host."
        )
    args.host_literal = literal


def http_binding(args: argparse.Namespace) -> tuple[str, int]:
    """The address the HTTP transport listens on, defaults filled in.

    One place resolves the ``None`` sentinels that let ``_check_transport``
    tell "the operator passed this" from "the operator said nothing", so the
    listener and the refusal can never disagree about what the default is.

    The host is the literal :func:`_check_transport` resolved and approved,
    carried on the namespace rather than resolved again. Two independent
    lookups between the loopback check and the socket -- this one and the
    server stack's own at bind time -- would let a name whose records change
    in between (a short TTL, a round-robin mixing loopback with a routable
    address) pass the check and bind the other answer. What was verified has
    to be what is used, so there is exactly one lookup and this reads its
    result.
    """
    port = args.port if args.port is not None else DEFAULT_HTTP_PORT
    return args.host_literal, port


def parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    """Parse the server's own command line."""
    parser = argparse.ArgumentParser(
        prog="agentless-mcp-server",
        description="Read-only MCP server over the agentless-mcp read surface.",
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s {server_version()}",
    )
    parser.add_argument(
        "--root",
        action="append",
        default=[],
        metavar="DIR",
        type=root_dir,
        help="a repository this server may serve; must exist at startup; repeatable",
    )
    parser.add_argument(
        "--roots-from",
        action="append",
        default=[],
        metavar="FILE",
        type=roots_file,
        help=(
            "a file of repository paths, one per line, added to --root; blank "
            "lines and #-comment lines are skipped and relative paths resolve "
            "against the working directory, as --root does; repeatable"
        ),
    )
    parser.add_argument(
        "--allow-client-roots",
        action="store_true",
        help=(
            "let the connected client's advertised MCP roots authorise "
            "repositories, not merely select among --root directories. This "
            "hands the client the operator's decision about what is servable"
        ),
    )
    parser.add_argument(
        "--no-auto-warm",
        action="store_true",
        help="do not warm cold grammars in the background at startup; grammars "
        f"then warm only through agentless-mcp warmup ({grammars.ENV_NO_AUTO_WARM} "
        "in the environment does the same)",
    )
    parser.add_argument(
        "--no-auto-index",
        action="store_true",
        help="do not refresh a stale tag cache in the background when a "
        "repository is first served; the cache then updates only through "
        f"agentless-mcp index ({cache.ENV_NO_AUTO_INDEX} in the environment "
        "does the same)",
    )
    parser.add_argument(
        "--no-auto-restart",
        action="store_true",
        help="do not restart the HTTP server when its installed package "
        "changes; the process then serves the code it loaded at startup "
        f"until restarted by hand ({selfrestart.ENV_NO_AUTO_RESTART} in the "
        "environment does the same)",
    )
    parser.add_argument(
        "--surface",
        choices=SURFACES,
        default=SURFACE_V2,
        help=(
            "which tool surface to publish: v2 (the default) is the five "
            "consolidated intent-shaped tools, v1 is the original eleven for "
            "un-migrated operators, both publishes the union for a transition "
            "window"
        ),
    )
    parser.add_argument(
        "--transport",
        choices=TRANSPORTS,
        default=TRANSPORT_STDIO,
        help=(
            "how the server talks to its client: stdio (the default) for a "
            "client that launches this process, http for one long-lived server "
            "several clients share over FastMCP's streamable-http transport"
        ),
    )
    parser.add_argument(
        "--host",
        default=None,
        metavar="ADDR",
        help=(
            f"address --transport {TRANSPORT_HTTP} binds; loopback only, "
            f"default {DEFAULT_HTTP_HOST}"
        ),
    )
    parser.add_argument(
        "--port",
        default=None,
        type=int,
        metavar="N",
        help=f"port --transport {TRANSPORT_HTTP} binds; {MIN_HTTP_PORT}-{MAX_HTTP_PORT}, "
        f"default {DEFAULT_HTTP_PORT}",
    )
    try:
        args = parser.parse_args(argv)
        # Inside the try on purpose: parser.error leaves by the same SystemExit
        # door argparse itself uses, so a transport refusal gets the argv
        # diagnostic below rather than reading to the operator as a dead socket.
        _check_transport(parser, args)
    except SystemExit as exc:
        # Under an MCP client the exit-2 usage error is invisible and the whole
        # session reads as a closed connection, so the argv itself is the
        # diagnostic. --help and --version leave by the same door with code 0.
        if exc.code not in (0, None):
            _report_argv(list(sys.argv[1:] if argv is None else argv))
        raise
    return args
