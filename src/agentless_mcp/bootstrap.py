"""Composition root: builds the object graph and hands it to an adapter.

Deliberately outside the import-linter layer contract -- it wires concrete
implementations into the adapters layer, so by construction it sits above
every layer and cannot be a member of one.

One extractor and one token counter per process, constructed here and passed
down. Nothing below this module reaches for a global: a service holds what it
was given, which is what makes the services testable with a stub counter and
what keeps parser memoization to one cache instead of one per call.

The MCP server is loaded by name rather than imported at module scope, because
``fastmcp`` lives behind the ``mcp`` extra and a CLI-only install must not pay
for it -- or fail at import time because of it. This is optional-dependency
gating, not cycle-breaking: the dependency direction here is one-way and the
missing-extra case produces an actionable message instead of a traceback.

The tiktoken counter is here for the same reason and one more: an optional
dependency imported anywhere below the adapters would put a third-party
package inside the dependency contract's core, and a model-free tool would
have acquired a tokenizer. Selecting it is opt-in twice over -- the extra has
to be installed *and* ``--token-counter tiktoken`` passed -- because the
chars/4 estimator is what every token regression pin was measured with, and a
counter that changed underneath them would move budgets nobody asked to move.

Only the CLI can ask for it. The MCP server declares no ``--token-counter``
flag, so :func:`mcp_main` always ends up with the chars/4 estimator. It still
goes through :func:`select_counter` rather than naming the estimator itself,
so there is one place that decides what a process counts with and one place a
server flag would have to be wired into.
"""

import importlib
import importlib.metadata
import socket
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from typing import cast

from agentless_mcp.adapters.cli.formatting import EXIT_USAGE, fail
from agentless_mcp.adapters.cli.main import CliServices, build_parser, run
from agentless_mcp.adapters.mcp.cliargs import DISTRIBUTION_NAME, parse_args
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.errors import AgentlessError, OperationFailed
from agentless_mcp.util.tokens import (
    COUNTER_TIKTOKEN,
    TOKEN_COUNTERS,
    Chars4Counter,
    TokenCounter,
)

SERVER_MODULE = "agentless_mcp.adapters.mcp.server"

# The encoding tiktoken is asked for when it is selected. Named explicitly
# rather than derived from a model string: this package has no model, and an
# encoding that changed with a model name would make budgets depend on a
# parameter nothing else here has.
TIKTOKEN_ENCODING = "cl100k_base"

# How long the encoding load may spend on a socket. tiktoken downloads the BPE
# ranks on a cold cache and offers no timeout of its own, so the process-wide
# socket default is the only bound reachable from here. Generous, because it
# bounds a one-off fetch of a few megabytes rather than a request in a loop.
TIKTOKEN_FETCH_TIMEOUT_SECONDS = 30.0


@contextmanager
def _socket_default_timeout(seconds: float) -> Iterator[None]:
    """Bound every socket this process opens in the block, then put it back.

    ``socket.setdefaulttimeout`` is process-wide, which makes *where* it is
    called the whole of whether it is safe. It belongs to a caller that knows
    no other socket in the process is open -- see :func:`cli_main` -- and not
    to whatever happens to need a bound, which is why it is a context manager
    here rather than a few lines inside :class:`TiktokenCounter`.
    """
    previous = socket.getdefaulttimeout()
    socket.setdefaulttimeout(seconds)
    try:
        yield
    finally:
        socket.setdefaulttimeout(previous)


class TiktokenCounter:
    """Counts tokens with tiktoken, for callers who have it installed.

    Constructing one may reach the network: on a cold cache tiktoken fetches
    the BPE ranks over HTTP and offers no timeout of its own. This class does
    not bound that, because the only bound reachable from here is the
    process-wide socket default, and whether writing it is safe depends on
    what else in the process holds a socket -- which is a fact about the
    caller, not about this class. A caller that must not hang builds one
    inside :func:`_socket_default_timeout`, as :func:`cli_main` does.

    Holds the encoding rather than looking it up per call: the lookup reads a
    data file, and a budget search calls ``count`` once per binary-search
    step.
    """

    __slots__ = ("_encoding",)

    def __init__(self) -> None:
        try:
            tiktoken = importlib.import_module("tiktoken")
        except ImportError as exc:
            message = (
                f"--token-counter {COUNTER_TIKTOKEN} needs the 'tokens' extra, which is not "
                f"installed ({exc}). Install it with: uv sync --extra tokens, or "
                "pip install 'agentless-mcp[tokens]'."
            )
            raise OperationFailed(message) from exc

        # Inside the boundary, because this is the one line in this module that
        # reaches the network: on a cold cache tiktoken fetches the ranks over
        # HTTP, which fails with whatever its transport raises, and an unknown
        # encoding or an unwritable cache directory fail here too. Every one of
        # them is a wiring failure the caller asked for by name, so every one of
        # them leaves as the error `cli_main` already knows how to report.
        try:
            self._encoding = tiktoken.get_encoding(TIKTOKEN_ENCODING)
        except Exception as exc:
            message = (
                f"--token-counter {COUNTER_TIKTOKEN} could not load the "
                f"{TIKTOKEN_ENCODING} encoding ({exc!r}). tiktoken downloads it on first "
                "use; point TIKTOKEN_CACHE_DIR at a writable directory holding it, or run "
                "without the flag to use the chars/4 estimator."
            )
            raise OperationFailed(message) from exc

    def count(self, text: str) -> int:
        """Return the number of tokens ``text`` encodes to."""
        return len(self._encoding.encode(text))


def select_counter(choice: str | None) -> TokenCounter:
    """Build the token counter one process will use.

    ``None`` is "nobody chose", which is the chars/4 estimator -- the default
    the pins are written against.

    A name that is not one of :data:`TOKEN_COUNTERS` is refused rather than
    quietly answered with the default. argparse ``choices`` already screens the
    command line, so the only caller who can reach this is a library caller or
    a future front door, and for them a silent fallback is the worst outcome:
    every budget in the answer would be estimated by a counter they did not
    ask for, and nothing in the receipt would say so.
    """
    if choice is None:
        return Chars4Counter()
    if choice not in TOKEN_COUNTERS:
        message = (
            f"unknown token counter {choice!r}; the counters this build has are "
            f"{', '.join(TOKEN_COUNTERS)}"
        )
        raise OperationFailed(message)
    if choice == COUNTER_TIKTOKEN:
        return TiktokenCounter()
    return Chars4Counter()


def counter_choice(argv: Sequence[str] | None) -> str | None:
    """Read ``--token-counter`` out of an argv the full parser accepts.

    The counter has to exist before the services do, and the services have to
    exist before ``run`` can parse for real, so this argv is parsed twice. It
    is the *full* parser both times, which is what makes the two answers agree
    about more than spelling: a flag in a position the subcommand tree rejects
    is a usage error here, exactly as it is there, and argparse exits before
    anything is constructed. A pre-parse that accepted more than the real one
    built a counter for a command line that was never going to run -- and
    reported a missing extra for a flag the CLI does not take in that position.
    """
    return cast("str | None", build_parser().parse_args(argv).token_counter)


def cli_main(argv: Sequence[str] | None = None) -> int:
    """Entry point for the ``agentless-mcp`` console script."""
    try:
        # argv first, so an inadmissible command line fails before anything is
        # built. Then the counter, under a bounded socket default: this line is
        # the composition root, no transport or index thread exists yet, and
        # the only thing inside the block that can open a socket is the
        # tiktoken encoding fetch. That is what makes writing a process-wide
        # setting safe here and nowhere further in.
        choice = counter_choice(argv)
        with _socket_default_timeout(TIKTOKEN_FETCH_TIMEOUT_SECONDS):
            counter = select_counter(choice)
    except AgentlessError as error:
        # `fail` rather than a second spelling of it, and EXIT_USAGE rather
        # than a number: an extra that is not installed makes the flag itself
        # inadmissible, which is the distinction `formatting` already owns.
        return fail(str(error), EXIT_USAGE)
    extractor = TreeSitterExtractor()
    patches = PatchService(extractor)
    services = CliServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        patches=patches,
        validates=ValidateService(patches),
        lints=LintService(extractor),
        counter=counter,
        extractor=extractor,
    )
    return run(argv, services)


def _mcp_extra_requirements() -> list[str]:
    """The requirement strings pyproject declares for the ``mcp`` extra.

    Read from the installed metadata rather than restated here, so the
    diagnosis below can never disagree with pyproject about which versions
    this build imports from. The marker spelling varies by build backend
    (single or double quotes), so both are matched.
    """
    try:
        requirements = importlib.metadata.requires(DISTRIBUTION_NAME) or []
    except importlib.metadata.PackageNotFoundError:
        return []
    markers = ("extra == 'mcp'", 'extra == "mcp"')
    return [
        requirement.split(";")[0].strip()
        for requirement in requirements
        if any(marker in requirement for marker in markers)
    ]


def _mcp_extra_diagnosis(exc: ImportError) -> str:
    """Say why the server module did not import, without lying about the fix.

    Two distinct failures used to share one message. When the extra is absent,
    installing it is the fix. When the extra is present but resolved to a
    major the code does not import from -- an upstream major landing on a
    fresh install, or an independent upgrade of fastmcp -- that same message
    sent the operator to a reinstall that reproduced the broken resolution.
    The two are told apart by the invariant itself: whether the distributions
    the extra declares are installed, not by the shape of the ImportError,
    because a removed submodule in a new major raises the same
    ModuleNotFoundError a missing package does.
    """
    installed: list[str] = []
    for name in ("fastmcp", "mcp", "pydantic"):
        try:
            installed.append(f"{name} {importlib.metadata.version(name)}")
        except importlib.metadata.PackageNotFoundError:
            return (
                f"the MCP server needs the 'mcp' extra, which is not installed ({exc}). "
                "Install it with: uv sync --extra mcp, or pip install 'agentless-mcp[mcp]'."
            )
    declared = ", ".join(_mcp_extra_requirements()) or "the versions pyproject.toml declares"
    return (
        f"the MCP server's dependencies are installed but not API-compatible ({exc}). "
        f"Found {', '.join(installed)}; this build needs {declared}. Reinstall the "
        "extra so the resolver applies those bounds: uv tool install --force "
        "'agentless-mcp[mcp]', or pip install --force-reinstall 'agentless-mcp[mcp]'."
    )


def mcp_main(argv: Sequence[str] | None = None) -> int:
    """Entry point for the ``agentless-mcp-server`` stdio MCP script."""
    # Parsed before the gated import, from the module that stays importable
    # without the extra, so --help, --version and usage errors answer on a
    # bare install instead of exiting 2 on the missing extra. `serve` parses
    # the same argv again with the same parser -- the pattern `counter_choice`
    # documents -- so the two reads cannot disagree about what is admissible.
    parse_args(argv)
    try:
        module = importlib.import_module(SERVER_MODULE)
    except ImportError as exc:
        return fail(_mcp_extra_diagnosis(exc), EXIT_USAGE)

    extractor = TreeSitterExtractor()
    # The server declares no `--token-counter`, so nobody chose -- which is
    # what `None` means here. Routed through the same selection the CLI uses
    # rather than naming the estimator, so a server flag would have one place
    # to arrive and both entry points cannot drift into different defaults.
    counter = select_counter(None)
    # module is Any to mypy, so neither these field names nor the cast below is
    # type-checked -- the dynamic import that gates the optional extra is what
    # hides the signatures. tests/unit/test_transport_e2e.py spawns the installed
    # console script, so a renamed field or a changed serve signature fails there
    # at startup instead of passing silently.
    services = module.ServerServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        counter=counter,
        extractor=extractor,
    )
    serve = cast("Callable[[Sequence[str] | None, object], int]", module.serve)
    return serve(argv, services)
