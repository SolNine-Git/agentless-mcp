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
agentless-mcp diagram > modules.mmd
agentless-mcp html > modules.html
```

These commands find relationships between symbols or files, report import
cycles, group related files, and export Mermaid or interactive HTML graphs.

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

Use `--no-cache` on repository-scoped commands to bypass the cache.

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

The MCP tools are:

| Tool | Purpose |
| --- | --- |
| `repo_map` | Rank relevant files and symbols within a token budget |
| `list_dir` | List the repository tree |
| `get_symbols_overview` | Show declarations without symbol bodies |
| `expand_symbols` | Return full symbol bodies |
| `read_slice` | Read selected source lines |
| `find_symbol` | Find symbols by name |
| `find_referencing_symbols` | Find references and callers |
| `explain_symbol` | Show a symbol with its relationships |
| `analyze_structure` | Query paths, cycles, communities, or diagrams |
| `resolve_locations` | Resolve class, function, and line locations |
| `capabilities` | Report loaded grammars and cache state |

The MCP server does not apply patches or execute repository commands.

## Supported languages

The bundled grammars support Bash, C, C++, C#, Go, HCL, Java, JavaScript, JSON,
Kotlin, Lua, PHP, Python, Ruby, Rust, Scala, SQL, Swift, TOML, TSX, TypeScript,
and YAML. Run `capabilities` to see the grammars available in the current
installation.

## License

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
