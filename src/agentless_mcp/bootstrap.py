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
"""

import importlib
import sys
from collections.abc import Callable, Sequence
from typing import cast

from agentless_mcp.adapters.cli.main import CliServices, counter_parser, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.errors import AtlasError
from agentless_mcp.util.tokens import (
    COUNTER_TIKTOKEN,
    Chars4Counter,
    TokenCounter,
)

SERVER_MODULE = "agentless_mcp.adapters.mcp.server"

# The encoding tiktoken is asked for when it is selected. Named explicitly
# rather than derived from a model string: this package has no model, and an
# encoding that changed with a model name would make budgets depend on a
# parameter nothing else here has.
TIKTOKEN_ENCODING = "cl100k_base"


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
            raise AtlasError(message) from exc
        self._encoding = tiktoken.get_encoding(TIKTOKEN_ENCODING)

    def count(self, text: str) -> int:
        """Return the number of tokens ``text`` encodes to."""
        return len(self._encoding.encode(text))


def select_counter(choice: str | None) -> TokenCounter:
    """Build the token counter one process will use.

    ``None`` is "nobody chose", which is the chars/4 estimator -- the default
    the pins are written against.
    """
    if choice == COUNTER_TIKTOKEN:
        return TiktokenCounter()
    return Chars4Counter()


def counter_choice(argv: Sequence[str] | None) -> str | None:
    """Read ``--token-counter`` out of the argv before the real parse.

    ``parse_known_args`` on a parser holding nothing else: the flag has to be
    read before the services exist, and the full parser cannot run until they
    do. The flag is declared once, in the adapter, and the full parser accepts
    it too, so this pre-parse can never disagree with it about spelling.
    """
    known, _ = counter_parser().parse_known_args(argv)
    return cast("str | None", known.token_counter)


def cli_main(argv: Sequence[str] | None = None) -> int:
    """Entry point for the ``agentless-mcp`` console script."""
    extractor = TreeSitterExtractor()
    try:
        counter = select_counter(counter_choice(argv))
    except AtlasError as error:
        sys.stderr.write(f"agentless-mcp: {error}\n")
        return 2
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
        sys.stderr.write(
            "agentless-mcp: the MCP server needs the 'mcp' extra, which is not installed "
            f"({exc}). Install it with: uv sync --extra mcp, or pip install "
            "'agentless-mcp[mcp]'.\n"
        )
        return 2

    extractor = TreeSitterExtractor()
    counter = Chars4Counter()
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
