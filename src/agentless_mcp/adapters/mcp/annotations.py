"""Tool annotations: every tool this server exposes is read-only.

The annotations are a claim a client can act on before it calls anything, and
the claim here is unconditional because the surface is: there is no write, no
exec and no fetch tool on this server, and the analysed repository is only
ever opened for reading. Patch application and test execution live on the CLI,
behind a git worktree, on purpose. The server's own derived-fact caches --
warmed grammars, the tag cache -- are the one thing background work refreshes,
and they live under the user cache directory, never inside the repository:
they change an answer's speed, not its content.

Verified against the installed FastMCP (3.4.7): ``@mcp.tool(annotations=...)``
accepts an ``mcp.types.ToolAnnotations`` model or a plain dict, and the fields
it carries are ``title``, ``readOnlyHint``, ``destructiveHint``,
``idempotentHint`` and ``openWorldHint``. ``idempotentHint`` is set as well as
the three the plan names: a read of an unchanged repository returns the same
answer, which is the property a client retrying a timed-out call needs.
"""

from typing import Any

READ_ONLY_ANNOTATIONS: dict[str, Any] = {
    "readOnlyHint": True,
    "destructiveHint": False,
    "idempotentHint": True,
    # No tool here reaches the network. The one component that ever can --
    # grammar download -- is a CLI warmup step, never a tool call.
    "openWorldHint": False,
}


def read_only(title: str) -> dict[str, Any]:
    """Return the read-only annotation set with a human-readable title."""
    return {"title": title, **READ_ONLY_ANNOTATIONS}
