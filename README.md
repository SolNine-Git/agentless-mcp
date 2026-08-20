# agentless-mcp

`agentless-mcp` is model-free structural machinery for coding agents, built
around evidence-tiered reference edges. Every name match is labelled
`same-file`, `resolved-via-import`, `unique`, or `name-only-ambiguous`, so a
caller can distinguish a local binding from a guess instead of receiving one
flat list that reads like ground truth. The same core provides a tree-sitter
repo map at any zoom level, symbol and reference lookup, resolved-graph views,
and deterministic patch validation. It never calls a language model; the
calling agent supplies all reasoning.

## Install

From PyPI:

```
uv tool install agentless-mcp
agentless-mcp warmup
```

Add the `mcp` extra for the stdio server
(`uv tool install "agentless-mcp[mcp]"`). Pin the package version when an
upgrade must be reviewable.

An optional, portable agent skill is provided at
[`docs/skills/agentless-mcp/SKILL.md`](docs/skills/agentless-mcp/SKILL.md).
Nothing installs it automatically. Copy its containing `agentless-mcp`
directory into the user skill root for the client you use:

| Client | User skill root |
|---|---|
| Claude Code | `~/.claude/skills/` |
| Codex | `~/.codex/skills/` |
| Cursor | `~/.cursor/skills/` |
| Gemini CLI | `~/.gemini/skills/` |
| OpenCode | `~/.config/opencode/skills/` |

The skill is a soft routing nudge, not a hook: it recommends literal search
when the string or file is known and the structural tools when location,
fan-in, blast radius, or a cross-file change surface is the question.

## Status

Phases 1 through 6 are in. The CLI and the stdio MCP server are both live over
one set of application services: repository map, directory tree, skeleton,
slice, symbol lookup, symbol expansion, fan-in, location resolution, and the
resolved-graph views — `explain` (one symbol's tiered fan-in and fan-out),
`path` (shortest resolved path between two symbols) and `cycles` (module-level
import cycles), plus an on-demand searchable HTML graph export. The write
side — SEARCH/REPLACE patch parsing, syntax
checking, worktree-isolated apply, candidate validation and the
equivalence-clustered vote — is CLI-only and never exposed over MCP.

Parsing happens on demand by default. `agentless-mcp index` builds an optional
per-repository SQLite cache under `$XDG_CACHE_HOME/agentless-mcp/`, never
inside the analyzed repository. Structural facts are keyed by each file's
SHA-256, so changed files are re-parsed while unchanged files are reused. A
`tree_oid` generation stamp and the receipt on every answer state whether the
cache still belongs to the repository generation being read. This is
incremental invalidation with generation state in the response envelope, not a
commit-hook snapshot: it remains correct while concurrent agents work on a
dirty tree, when a commit-refreshed artifact would be stale for the entire
work session. `--no-cache` / `no_cache: true` bypasses it.

The design rule is: **cache only what a hash can invalidate; recompute
everything else at read time or do not store it.** That is why the package has
no model-authored semantic layer. Incorrect generated prose is not merely
stale; it is unfalsifiable from the source hash if it was wrong when written.
The capability gradient also runs backwards when a cheaper model authors
premises a stronger model later consumes as fact, and the blast radius spans
every agent and future session that re-reads the premise.

A repository may declare its own defaults in an optional `.agentless-mcp.json`
at its root (map budget, max files, granularity, docstrings, stoplist
additions, and a `test_cmd` that only the CLI's `validate` will use, only when
the invocation names none of its own and passes `--allow-repo-test-cmd`, and
only after printing it). The file is
repository content: every value is schema-checked and bounded, no key is
path-typed, and unknown keys are warnings in the response envelope rather than
errors.

## Language servers and adjacent graph tools

This is not a language server. Name-plus-import binding is softer than
type-aware resolution, especially for TypeScript barrel files, re-exports and
duck-typed method calls; the evidence tiers expose that limit instead of
hiding it. In exchange, the machinery works on code that does not build, uses
one interface across every supported language, needs no server lifecycle or
project build, and can read many repositories in one process.

[Graphify](https://github.com/Graphify-Labs/graphify) covers a much broader
surface: roughly forty code languages plus SQL, configs, documents, PDFs and
media, with an interactive `graph.html` and a human-readable report. Its code
graph is also deterministic tree-sitter extraction without model calls, so
model-freeness is not a differentiator there. `agentless-mcp` instead focuses
on freshness during concurrent uncommitted work, four-way binding evidence,
and never writing artifacts into the analyzed repository. The tools are
complements, not interchangeable replacements.

The CLI-only write side is multi-agent arbitration. When several independent
agents propose fixes, SEARCH/REPLACE parsing, AST-equivalence keys, bounded
validation and equivalence-clustered voting answer whether candidates are the
same fix and which surviving class has the most support. It is not the primary
workflow for a single interactive agent, and none of it is exposed through
MCP.

Start with [docs/agent-guide.md](docs/agent-guide.md): it is the usage guide
written for the agent that will call this. The 60-task navigation results,
selection counts, spread, and denominator caveats are in
[docs/evidence.md](docs/evidence.md).

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

Read-only and path-confined does not mean secret-blind. The map and tree honor
gitignore, but directed tools such as `read_slice` and
`get_symbols_overview` accept an explicit path inside the root. They can
therefore surface ignored `.env` files, `.git/config`, credentials and any
other file the calling user can read. Treat repository output as sensitive and
apply the same access controls you would to an agent's ordinary file-read tool.

`validate` test commands receive an explicit environment containing only
`PATH`, `HOME`, `LANG` and `TMPDIR` when those variables exist. Additional
names require a separate `--pass-env NAME` flag for each variable. This limits
accidental credential inheritance; it is not a sandbox. The command still runs
as the calling user and can read files available to that user. A `test_cmd`
from `.agentless-mcp.json` is also refused unless
`--allow-repo-test-cmd` is present.

One supply-chain caveat is worth stating up front. Grammars come from
`tree-sitter-language-pack`, which downloads a prebuilt platform bundle on first
use. Those bundles are SHA-256 verified against a release manifest, and a
mismatch is a hard failure — but the manifest itself (`parsers.json`) is
HTTPS-trusted rather than cryptographically signed. We accept that risk under
exact release-version pinning: `tree-sitter` and `tree-sitter-language-pack` are
pinned as a unit and move only through a reviewed bump. The recommended
operational posture is to pre-seed the grammar cache once with
`agentless-mcp warmup`, review that exposure, then set
`AGENTLESS_MCP_NO_DOWNLOAD=1` for normal operation. Fetching is confined to an
explicit warmup step, and both the manifest URL and the cache directory are
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
