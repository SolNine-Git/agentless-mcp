# agentless-mcp

`agentless-mcp` is model-free structural machinery for coding agents: an
on-demand tree-sitter repo map at any zoom level (directory tree, skeleton,
line slice), symbol and reference lookup for bug localization, and
deterministic patch parsing, syntax-checking and candidate validation. It
never calls a language model — the calling agent supplies all the reasoning,
and this tool supplies the parsing, ranking, containment and verification it
would otherwise improvise. It ships as a CLI any agent can invoke over Bash and
as a thin stdio MCP server over the same core.

## Install

Today, from git:

```
uv tool install git+https://github.com/SolNine-Git/agentless-mcp
agentless-mcp warmup
```

Add the `mcp` extra for the stdio server
(`uv tool install "agentless-mcp[mcp] @ git+..."`). A PyPI release will follow;
until then the git URL is the install route.

## Status

Phases 1 through 6 are in. The CLI and the stdio MCP server are both live over
one set of application services: repository map, directory tree, skeleton,
slice, symbol lookup, symbol expansion, fan-in, location resolution, and the
resolved-graph views — `explain` (one symbol's tiered fan-in and fan-out),
`path` (shortest resolved path between two symbols) and `cycles` (module-level
import cycles). The write side — SEARCH/REPLACE patch parsing, syntax
checking, worktree-isolated apply, candidate validation and the
equivalence-clustered vote — is CLI-only and never exposed over MCP.

Reference resolution is deterministic and model-free: a name is bound to its
candidate definitions through the file's own imports and its own definitions,
and every edge carries the discrete evidence tier behind it (same-file,
resolved-via-import, unique, name-only-ambiguous). The graph is assembled in
memory on every call from the current tree — nothing about it is stored, and
there is no watcher.

Parsing happens on demand by default; `agentless-mcp index` builds a
per-repository SQLite cache under `$XDG_CACHE_HOME/agentless-mcp/` (never
inside the analyzed repository) holding symbols, imports and references for
files whose sha256 has not moved. Every answer's receipt names the cache
generation and whether it is still the repository's own, and `--no-cache` /
`no_cache: true` forces on-demand parsing.

A repository may declare its own defaults in an optional `.agentless-mcp.json`
at its root (map budget, max files, granularity, docstrings, stoplist
additions, and a `test_cmd` that only the CLI's `validate` will use, only when
the invocation named none of its own, and only after printing it). The file is
repository content: every value is schema-checked and bounded, no key is
path-typed, and unknown keys are warnings in the response envelope rather than
errors.

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

## Windows support

Linux and macOS are the supported platforms: they are what the test suite runs
on, and every guarantee below is asserted there. Windows is **documented best
effort** — the POSIX-only calls have Windows paths, there is no Windows CI, and
nothing here has been executed on Windows.

Covered:

- The whole read surface. Map, tree, skeleton, slice, symbol lookup, fan-in and
  location resolution are pure parsing and path arithmetic, and paths go
  through `pathlib` with repository-relative results normalized to forward
  slashes.
- The tag cache, including its single-writer discipline. The index lock is
  `fcntl.flock` on POSIX and `msvcrt.locking` on Windows, selected in
  `util/filelock.py`; both are non-blocking, so a second concurrent index run
  is refused rather than queued on either platform.
- Patch parsing, syntax checking and the equivalence key.

Not covered, and the difference is real:

- **The timeout guarantee is weaker.** On POSIX a timed-out test command has
  its whole process group killed (SIGTERM, then SIGKILL), so anything it
  spawned dies with it. On Windows the child is created with
  `CREATE_NEW_PROCESS_GROUP` and then `terminate()`/`kill()` is sent to the
  leader only: **grandchildren can survive a timeout**. If your test command
  starts a server, expect to clean it up yourself.
- **No Windows CI.** The platform dispatch is unit-tested on Linux; the
  Windows system calls themselves are not exercised anywhere.
- Anything depending on `git worktree` behaves as the local git does; it has
  not been exercised on Windows filesystems.

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

## License

MIT — see [LICENSE](LICENSE).

Parts of this package are derived from
[Agentless](https://github.com/OpenAutoCoder/Agentless) (MIT, Copyright (c)
2024 OpenAutoCoder): the line-slice primitives, the location grammar, the
SEARCH/REPLACE edit format and its parser, and the candidate filter ladder and
vote key. [NOTICE](NOTICE) names each file and reproduces the upstream
license.
