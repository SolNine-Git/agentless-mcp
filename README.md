# agentless-mcp

`agentless-mcp` provides tree-sitter-based code navigation, repository
structure analysis, and patch validation for local repositories. It exposes
the same functionality through a command-line interface and a read-only
MCP server.

## Install

```sh
uv tool install agentless-mcp
```

That installs two console scripts. `agentless-mcp` is the CLI, for a human or
for an agent driving it over a shell. `agentless-mcp-server` is the MCP
server, which an MCP client launches for you rather than something you run in
a terminal; it needs the `mcp` extra, so install that when you want the server:

```sh
uv tool install "agentless-mcp[mcp]"
```

Both entry points warm cold grammars in the background at startup (one
digest-verified bundle fetch at most; `--no-auto-warm` or
`AGENTLESS_MCP_NO_AUTO_WARM` opts out, `AGENTLESS_MCP_NO_DOWNLOAD` forbids
all fetching). To warm explicitly and fail loudly instead:

```sh
agentless-mcp warmup
```

`agentless-mcp guide` prints the full agent usage guide, which ships with the
package; `agentless-mcp guide --section NAME` prints one section, and an
unknown name lists them all.

## CLI

Most commands analyze the repository containing the current directory. Use
`--repo PATH` to select another repository. Add `--json` where supported for
machine-readable output.

### Navigate code

```sh
agentless-mcp map --focus src/app.py
agentless-mcp tree --depth 3
agentless-mcp skeleton src/app.py
agentless-mcp expand py:src/app.py::App.run
agentless-mcp slice src/app.py --lines 40:80
agentless-mcp find-symbol App
agentless-mcp refs App.run
agentless-mcp explain App.run
```

These commands provide repository maps, directory trees, symbol overviews,
full symbol bodies, source slices, symbol lookup, references, and symbol
context. Symbol IDs are printed by `map` and `skeleton` and can be passed to
`expand`, `refs`, `explain`, and related commands.

### Analyze structure

```sh
agentless-mcp path App.run Database.connect
agentless-mcp cycles
agentless-mcp communities
agentless-mcp health
agentless-mcp diagram > modules.mmd
agentless-mcp html > modules.html
```

These commands find relationships between symbols or files, report import
cycles, group related files, list orphan candidates, unused exports and hubs,
and export Mermaid or interactive HTML graphs.

### Validate patches

The CLI also supports deterministic patch workflows:

```sh
agentless-mcp patch parse --file change.patch
agentless-mcp patch check --file change.patch --repo /path/to/repo
agentless-mcp patch apply --file change.patch --repo /path/to/repo
agentless-mcp lint --candidates ./candidates --repo /path/to/repo
agentless-mcp lint --diff change.patch --repo /path/to/base-checkout
agentless-mcp validate --candidates ./candidates --repo /path/to/repo \
  --test-cmd 'pytest -q'
agentless-mcp vote --verdicts verdicts.jsonl
```

Patch candidates can use SEARCH/REPLACE text or the package's `edits.json`
format. `validate` runs candidates against the repository tests, and `vote`
ranks the candidates that pass.

`lint --diff` runs the same checks over a branch's or a pull request's unified
diff, so a change that already exists does not have to be hand-converted first.
The checks compare the diff against `--repo` as it stands, which means **`--repo`
must be a checkout of the diff's base, not a tree with the diff already
applied** — otherwise every symbol the diff adds is already in the file and the
report would describe the change against itself. The usual shape is a second
worktree at the merge-base:

```sh
git diff main...HEAD > change.patch
git worktree add /tmp/base $(git merge-base main HEAD)
agentless-mcp lint --diff change.patch --repo /tmp/base
```

Pointing `--repo` at the branch instead is not silently wrong: each affected
file is reported as a `not_checked` coverage gap naming the remedy. Binary files
and mode-only changes are reported the same way, and a construct one edit cannot
express — a rename, a `-U0` diff with no context — is refused with the reason.

### Cache and capabilities

Parsing happens on demand. Build an optional repository cache to improve
repeated queries:

```sh
agentless-mcp index --repo /path/to/repo
agentless-mcp capabilities --repo /path/to/repo
```

The MCP server builds and refreshes this cache itself, in the background,
the first time it serves a repository whose index is absent or stale
(`--no-auto-index` or `AGENTLESS_MCP_NO_AUTO_INDEX` opts out); the CLI
indexes only through the explicit command above. Use `--no-cache` on
repository-scoped commands to bypass the cache.

## MCP server

The server exposes read-only repository tools. It talks over stdio by
default, which is what a client that launches the server as a child expects.
For a single-user machine, register it once and let the client's advertised
workspace authorize repositories: whatever repository you open a session in
is served on the first tool call, with nothing to enable per repo.

```sh
claude mcp add --scope user agentless -- agentless-mcp-server --allow-client-roots
```

For a locked-down server, omit `--allow-client-roots` and pass an explicit
allowlist instead; then only the listed repositories are servable, and a
client-advertised root can only select among them, never add one:

```sh
agentless-mcp-server --root /path/to/repo --root /path/to/other
```

Over HTTP the server also watches its own install: when the package is
upgraded or reinstalled, it finishes in-flight requests and replaces itself
with the new code (`--no-auto-restart` or `AGENTLESS_MCP_NO_AUTO_RESTART`
opts out). A long-running process otherwise serves the code it loaded at
startup forever -- reconnecting clients refreshes the connection, never the
process. On Windows the server exits cleanly instead and a supervisor's
`Restart=` completes the loop; `docs/deploy/mcp-agentless.service` is a
ready example unit.

`--roots-from FILE` reads that same list from a file, one path per line.
The file is re-read whenever it changes on disk, so appending a line enrolls
a repository on the next call without a restart, and the refusal an agent
sees for an unlisted repository names the file to append to. Blank lines and
whole-line `#` comments are skipped, and the flag is repeatable and combines
with `--root`:

```sh
cat > ~/.config/agentless-mcp/roots <<'EOF'
# one repository path per line
/path/to/repo
/path/to/other
EOF
claude mcp add --scope user agentless -- \
  agentless-mcp-server --roots-from ~/.config/agentless-mcp/roots
```

### Serving over HTTP

A client that cannot spawn a child process gets the same tools over FastMCP's
streamable-http transport, from one long-lived server that several clients
share. The endpoint is `http://HOST:PORT/mcp`:

```sh
agentless-mcp-server --transport http --port 8766 \
  --roots-from ~/.config/agentless-mcp/roots
```

The bind address is loopback-only and is checked, not merely defaulted: this
server authenticates nobody, so the `--root` allowlist decides which
repositories are readable and says nothing about who may read them. On a
routable address that is unauthenticated read access to every enrolled
repository, so a non-loopback `--host` is refused before the socket opens.
Put an authenticating proxy in front if you need it off-host.

`--host` and `--port` apply to the HTTP transport only; passing either under
stdio is refused rather than ignored, because there is no socket to bind.

Every tool takes `repo_root` first. It may be omitted only when the server
holds one repository, or when the client advertises a root that selects
exactly one; otherwise the refusal lists the roots to choose from.

The MCP tools are five intent-shaped surfaces; three of them fold their
questions behind an `operation` parameter:

| Tool | Operations | Purpose |
| --- | --- | --- |
| `orient` | `map`, `communities`, `cycles`, `diagram`, `path`, `health` | Where does this live, how is the repository put together |
| `symbols` | `find`, `overview`, `expand`, `explain`, `locate` | Look up, skeleton, expand, or explain symbols; resolve locations |
| `find_referencing_symbols` | | Find references and callers (blast radius) |
| `read` | `slice`, `dir` | Read selected source lines; list the repository tree |
| `capabilities` | | Report loaded grammars and cache state |

One worked call per surface:

```
orient(operation="map", focus=["src/app.py", "quote"])
symbols(operation="expand", stable_ids=["py:src/app.py::App.run"])
find_referencing_symbols(target="App.run")
read(operation="slice", path="src/app.py", lines=[[40, 80]])
capabilities()
```

A wrong `operation` is answered with the valid list, and a parameter foreign
to the selected operation is refused with a message naming what that
operation accepts and requires.

This v2 surface is the default. For the transition, `--surface v1` publishes
the previous per-question tools (`repo_map`, `expand_symbols`, and the rest)
and `--surface both` publishes the union; v1 remains for one release. The
mapping between the surfaces is in
`agentless-mcp guide --section the-two-surfaces`.

The MCP server does not apply patches or execute repository commands.

### Keeping the tools enabled in Claude Code

Claude Code asks for approval the first time a session calls each MCP tool.
Every prompt is a chance to fall back to `Grep`, and the approval does not
carry to the next session. Pre-approve the server once instead, in
`permissions.allow` in `~/.claude/settings.json` for every project, or in
the repository's `.claude/settings.json` for one:

```json
{
  "permissions": {
    "allow": [
      "mcp__agentless__orient",
      "mcp__agentless__symbols",
      "mcp__agentless__find_referencing_symbols",
      "mcp__agentless__read",
      "mcp__agentless__capabilities"
    ]
  }
}
```

A rule of the form `mcp__agentless` approves every tool the server
publishes, which survives a surface change and a new operation. List the
tools one by one, as above, when you want each addition to ask once before
it runs unattended. Claude Code matches these rules literally: a wildcard
such as `mcp__agentless__*` matches nothing.

The prefix carries the server name you registered. These entries assume the
`claude mcp add ... agentless ...` line at the top of this section. Under
`--surface v1` or `--surface both`, add the per-question tool names as
well: `repo_map`, `list_dir`, `get_symbols_overview`, `expand_symbols`,
`read_slice`, `find_symbol`, `explain_symbol`, `analyze_structure`, and
`resolve_locations`.

### Loading the tool schemas at session start

Approval is not the only way a session loses the server. Claude Code can
also defer an MCP server's tools: they arrive as bare names, and the model
must fetch each schema before it can call the tool. A deferred tool is not a
callable tool. `Grep` loads from the first turn, so the model routes a
locate step to `Grep` and never reaches this server.

Tell the model to load the schemas first, and tell it when to prefer them.
Both instructions belong in `CLAUDE.md`: use `~/.claude/CLAUDE.md` for every
project, or the repository's own `CLAUDE.md` for one.

````markdown
## Session Start

If the `mcp__agentless__*` tools show as deferred names rather than loaded
tools, load their schemas before the first code-locating action of the
session:

`ToolSearch(query="select:mcp__agentless__orient,mcp__agentless__symbols,mcp__agentless__find_referencing_symbols,mcp__agentless__read")`

A grep issued while these schemas sit unloaded is a routing failure, not a
neutral default. Every subagent dispatch prompt that navigates code must
include the same ToolSearch line. A worker without loaded schemas defaults
to `Grep`, because `Grep` is loaded from the start.

## Repo Navigation

The `agentless` MCP server owns structural navigation: where code lives,
what a file declares, who calls a symbol. Use it for every locate, trace,
or orient step in a repository it covers. `Grep` owns exact text: string
literals, error messages, config keys, and comments. A name that lives in
configuration or a fixture never appears in a symbol map, so run both
passes.

- Locate a bug or feature: `orient` (map) with focus seeds from the ask,
  then `symbols` (expand) for the few bodies that matter.
- See what a file declares: `symbols` (overview), never a whole-file read.
- Trace callers and blast radius: `find_referencing_symbols`.
- Check whether a symbol or utility exists: `symbols` (find).
- Orient in an unfamiliar repository: `orient` (communities), then `read`
  (dir). The map returns ten ranked files. It localizes, it does not
  enumerate.
````

Write the routing half as well as the loading half. A model that loads the
schemas and reads no routing rule still reaches for `Grep`.

## Supported languages

The bundled grammars support Bash, C, C++, C#, Go, HCL, Java, JavaScript, JSON,
Kotlin, Lua, PHP, Python, Ruby, Rust, Scala, SQL, Swift, TOML, TSX, TypeScript,
and YAML. Run `capabilities` to see the grammars available in the current
installation.

## License

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
