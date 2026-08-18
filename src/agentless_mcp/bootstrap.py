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
"""

import importlib
import sys
from collections.abc import Callable, Sequence
from typing import cast

from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.tokens import Chars4Counter

SERVER_MODULE = "agentless_mcp.adapters.mcp.server"


def cli_main(argv: Sequence[str] | None = None) -> int:
    """Entry point for the ``agentless-mcp`` console script."""
    extractor = TreeSitterExtractor()
    counter = Chars4Counter()
    patches = PatchService(extractor)
    services = CliServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor),
        patches=patches,
        validates=ValidateService(patches),
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
    services = module.ServerServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor),
        counter=counter,
        extractor=extractor,
    )
    serve = cast("Callable[[Sequence[str] | None, object], int]", module.serve)
    return serve(argv, services)
