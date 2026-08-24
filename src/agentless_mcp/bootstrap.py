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
import socket
from collections.abc import Callable, Sequence
from typing import cast

from agentless_mcp.adapters.cli.formatting import EXIT_USAGE, fail
from agentless_mcp.adapters.cli.main import CliServices, build_parser, run
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


class TiktokenCounter:
    """Counts tokens with tiktoken, for callers who have it installed.

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
        #
        # The socket default is set around the call and restored after it. It is
        # a blunt instrument -- it applies to whatever else opens a socket in
        # this window -- but nothing else in this process has started one yet,
        # and an unreachable network must not hang a CLI before it has parsed
        # its own argv. A warm cache never opens a socket at all.
        previous_timeout = socket.getdefaulttimeout()
        socket.setdefaulttimeout(TIKTOKEN_FETCH_TIMEOUT_SECONDS)
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
        finally:
            socket.setdefaulttimeout(previous_timeout)

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
        counter = select_counter(counter_choice(argv))
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


def mcp_main(argv: Sequence[str] | None = None) -> int:
    """Entry point for the ``agentless-mcp-server`` stdio MCP script."""
    try:
        module = importlib.import_module(SERVER_MODULE)
    except ImportError as exc:
        return fail(
            f"the MCP server needs the 'mcp' extra, which is not installed ({exc}). "
            "Install it with: uv sync --extra mcp, or pip install 'agentless-mcp[mcp]'.",
            EXIT_USAGE,
        )

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
