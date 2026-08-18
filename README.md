# agentless-mcp

`agentless-mcp` is model-free structural machinery for coding agents: an
on-demand tree-sitter repo map at any zoom level (directory tree, skeleton,
line slice), symbol and reference lookup for bug localization, and
deterministic patch parsing, syntax-checking and candidate validation. It
never calls a language model — the calling agent supplies all the reasoning,
and this tool supplies the parsing, ranking, containment and verification it
would otherwise improvise. It ships as a CLI any agent can invoke over Bash and
as a thin stdio MCP server over the same core.

## Status

Phase 1 read surface, index-free. The CLI and the stdio MCP server are both
live over one set of application services: repository map, directory tree,
skeleton, slice, symbol lookup, symbol expansion, fan-in and location
resolution. Parsing happens on demand — the tag cache is Phase 1.5, and
`agentless-mcp index` says so rather than pretending. The patch, validation and
vote machinery (Phases 2 and 3) is not implemented.

Start with [docs/agent-guide.md](docs/agent-guide.md): it is the usage guide
written for the agent that will call this.

## Development

```
uv sync --extra mcp
uv run agentless-mcp warmup
uv run pre-commit run --all-files
uv run pytest
```

`uv run pre-commit run --all-files` is the gate — it is a strict superset of
running ruff by hand (it adds ruff-format, mypy strict, codespell,
import-linter and deptry).

## Security

The tool's posture toward an analyzed repository is **read-only and
path-confined**: the MCP surface exposes read tools only, patch application and
test execution are CLI-side and default to an isolated git worktree, and no
cache or scratch state is written inside the repository under analysis.

One supply-chain caveat is worth stating up front. Grammars come from
`tree-sitter-language-pack`, which downloads a prebuilt platform bundle on first
use. Those bundles are SHA-256 verified against a release manifest, and a
mismatch is a hard failure — but the manifest itself (`parsers.json`) is
HTTPS-trusted rather than cryptographically signed. We accept that risk under
exact release-version pinning: `tree-sitter` and `tree-sitter-language-pack` are
pinned as a unit and move only through a reviewed bump. Fetching is confined to
an explicit warmup step, and both the manifest URL and the cache directory are
overridable for mirrored or air-gapped installs. See
[docs/supply-chain-audit.md](docs/supply-chain-audit.md) for the full audit.
