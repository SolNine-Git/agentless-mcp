# agentless-mcp

`agentless-mcp` provides tree-sitter-based code navigation, repository
structure analysis, and patch validation for local repositories. It exposes
the same functionality through a command-line interface and a read-only
MCP server.

## Install

```sh
uv tool install "agentless-mcp==0.11.0rc1"
```

0.11.0rc1 is a pre-release, the first Rust build. Until 0.11.0, a plain
`uv tool install agentless-mcp` or `uvx agentless-mcp` installs the Python
build 0.9.1, so name the version to get this one.

That installs two native commands. `agentless-mcp` is the CLI, for a human or
for an agent driving it over a shell. `agentless-mcp-server` is the MCP
server, which an MCP client launches for you rather than something you run in
a terminal. The wheel holds no Python package. The `mcp` and `tokens` extras
are empty, and they stay so that an install command of an earlier release,
such as `uv tool install "agentless-mcp[mcp]"`, still works.

The wheels cover Linux x86_64 and aarch64 with glibc 2.34 or later, and macOS
arm64 and x86_64. On another platform the install builds the sdist, which
needs a Rust toolchain and a C compiler.

In a local checkout, the root `pyproject.toml` builds no package. Build the
wheel from `packaging/`, then install the wheel file:

```sh
bash packaging/notices.sh
(cd packaging && maturin build --release --locked --out ../dist)
uv tool install --force --reinstall dist/agentless_mcp-*.whl
```

`notices.sh` needs cargo-about and GNU tar with zstd. Add `--reinstall`
whenever you rebuild: `uv tool install --force` can keep the installed build
while the version string is unchanged.

The `history` view needs git 2.25 or newer, the first release whose `git log
-L` documents `--no-patch` as suppressing the patch. On an older git the patch
text reaches the log parser, which refuses the answer rather than misread it.
Every other view works without git.

The 22 grammars are linked into `agentless-mcp`, so nothing is downloaded.
`agentless-mcp-server` is a small launcher that runs `agentless-mcp`. `--no-auto-warm`, `AGENTLESS_MCP_NO_AUTO_WARM` and
`AGENTLESS_MCP_NO_DOWNLOAD` are accepted and have no effect. To list the
linked grammars and probe each one:

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
agentless-mcp history py:src/app.py::App.run
```

These commands provide repository maps, directory trees, symbol overviews,
full symbol bodies, source slices, symbol lookup, references, symbol context,
and the commits that touched a symbol's lines. Symbol IDs are printed by `map`
and `skeleton` and can be passed to `expand`, `refs`, `explain`, and related
commands.

`history` takes a symbol id and prints the commits that changed that symbol's
lines, newest first, with their message bodies. `--limit` caps the commits
(default 10) and `--budget` caps the tokens spent on the bodies (default
12000); at most 120 commits render, however large the limit is. A span no
commit touched, a file that is not in HEAD, and a directory without git are
each a refusal rather than an empty answer.

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
cycles, group related files, list orphan candidates, unused exports, hubs and
complexity hotspots, and export Mermaid or interactive HTML graphs.

### Validate patches

The CLI also supports deterministic patch workflows:

```sh
agentless-mcp patch parse --file change.patch
agentless-mcp patch check --file change.patch --repo /path/to/repo
agentless-mcp patch apply --file change.patch --repo /path/to/repo
agentless-mcp lint --candidates ./candidates --repo /path/to/repo
agentless-mcp lint --diff change.patch --repo /path/to/base-checkout
agentless-mcp lint --path pkg/app.py --after /tmp/app.py --repo /path/to/repo
agentless-mcp lint --since "$(git merge-base HEAD main)" --repo /path/to/repo
agentless-mcp validate --candidates ./candidates --repo /path/to/repo \
  --test-cmd 'pytest -q'
agentless-mcp vote --verdicts verdicts.jsonl
```

Patch candidates can use SEARCH/REPLACE text or the package's `edits.json`
format. One request carries at most 500 edits and 2,000,000 bytes of search
and replace text; a larger one is refused when it is read, before any file is
opened. `validate` normalizes them against HEAD, runs byte-identical resulting
file states once while preserving every candidate's vote, and skips a
reproduction command when regression has already failed. `vote` ranks the
candidates that pass.

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

`lint --only CHECK[,CHECK...]` runs only the named checks. A run that names only
pair checks skips the repository scan that the patch checks need.
`complexity_delta` and `contract_drift` read only the edited files.
`clone_delta` also reads every source file outside test and fixture paths, to
find the copies of a block. `lint --path REL --after FILE` is the per-edit form.
It compares `REL` as the working tree holds it with the whole proposed text in
`FILE`, and it runs only the pair checks. The erosion gate hook calls lint this
way. `lint --since REV` is the review mode. It compares each file changed since
the commit `REV` with its text in the working tree, and it runs only the pair
checks. Both forms run `clone_delta` only when `--only` names it. Fixture
paths, `testdata` directories included, are outside every check, and test
paths are outside every check but `complexity_delta`.

By default `lint` exits 0 whatever it found. For a CI gate, `--fail-on
SEVERITY[,SEVERITY...]` (`warning`, `advisory`, `not_checked`) exits 3 when a
finding has a listed severity.

```sh
agentless-mcp lint --repo /path/to/repo --path pkg/app.py --after /tmp/app.py \
  --only complexity_delta --json
```

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

Every index run then releases cold repository caches until
`$XDG_CACHE_HOME/agentless-mcp/` fits under 5 GiB, least-recently-used first.
Because the server indexes on its own, nothing else bounds that directory.
Caches used in the last 24 hours are never released, so a working set larger
than the ceiling exceeds it instead of thrashing. Set
`AGENTLESS_MCP_MAX_CACHE_BYTES` to another ceiling, or to `0` to keep every
cache forever. An evicted repository loses no answers, only the speed: the
next call parses on demand and the one after that re-indexes.

The sweep counts and releases only the `agentless.db` files that this
release writes. It does not count or delete the `tags.db` files of the
Python releases, and this release does not read them. Delete them by hand.

## MCP server

The server exposes read-only repository tools. It talks over stdio by
default, which is what a client that launches the server as a child expects.
The server serves only the repositories that its `--root` flags and
`--roots-from` files name. Register it once with every repository you work
in:

```sh
claude mcp add --scope user agentless -- \
  agentless-mcp-server --root /path/to/repo --root /path/to/other
```

The server does not read the client's workspace roots. `--allow-client-roots`
is accepted and has no effect, so a server started with no roots refuses
every call.

Over HTTP the server also watches its own install: when the package is
upgraded or reinstalled, it finishes in-flight requests and replaces itself
with the new code (`--no-auto-restart` or `AGENTLESS_MCP_NO_AUTO_RESTART`
opts out). A long-running process otherwise serves the code it loaded at
startup forever -- reconnecting clients refreshes the connection, never the
process. `docs/deploy/mcp-agentless.service` is a ready example unit.

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

A client that cannot spawn a child process gets the same tools over the MCP
streamable HTTP transport, from one long-lived server that several clients
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

The server also refuses a request that a web page could send through DNS
rebinding. A request whose `Host` header is not `localhost`, `127.0.0.1`,
`[::1]` or the bound address gets `403 Forbidden`. So does a request whose
`Origin` header is present and names another host. A client that connects
by one of those names, with any port, is served.

`--host` and `--port` apply to the HTTP transport only; passing either under
stdio is refused rather than ignored, because there is no socket to bind.

Several clients sharing one server also share its worker threads, so
`--max-concurrency N` bounds how many tool calls run at once; it accepts 1 to
64 and defaults to 8. Every admitted call may hold a tag-cache connection and
a git child. Raise it for a server with many clients, lower it to keep the
machine's git and disk load predictable. It applies to stdio too, where the
default is already the right answer for a single client.

`--call-limit SECONDS` bounds how long one tool call may work, and defaults to
30. Time a call spends parsing files the tag cache does not hold is added on
top, up to `--recache-grace SECONDS` (default 60), so a call that rebuilds a
cold cache is not stopped for that. A call past its limit stops at its next
checkpoint and returns a refusal that names both bounds. A call the client
cancels stops the same way, so its worker thread does not run on after the
client has gone. On a clone of mcp-local (467 files) with a fresh tag cache,
a steady-state `orient` `map` call over stdio takes 55 ms and a
`find_referencing_symbols` call takes 47 ms (medians of 60 calls), well
inside the 30 s default. The CLI has no limit.

Every tool takes `repo_root` first. It may be omitted when the server holds
one repository. Under stdio it may also be omitted when the server's launch
directory is inside exactly one of its roots, and the call then gets that
root. A client that launches the server in the project directory therefore
needs no `repo_root`. Otherwise the refusal lists the roots to choose from.
Over HTTP the launch directory is the service's own, so a server with
several roots always needs `repo_root`.

The MCP tools are six intent-shaped surfaces; three of them fold their
questions behind an `operation` parameter:

| Tool | Operations | Purpose |
| --- | --- | --- |
| `orient` | `map`, `communities`, `cycles`, `diagram`, `path`, `health` | Where does this live, how is the repository put together |
| `symbols` | `find`, `overview`, `expand`, `explain`, `locate` | Look up, skeleton, expand, or explain symbols; resolve locations |
| `find_referencing_symbols` | | Find references and callers (blast radius) |
| `history` | | Why does this code exist: the commits that touched a symbol's lines, bodies included |
| `read` | `slice`, `dir` | Read selected source lines; list the repository tree |
| `capabilities` | | Report loaded grammars and cache state |

One worked call per surface:

```
orient(operation="map", focus=["src/app.py", "quote"])
symbols(operation="expand", stable_ids=["py:src/app.py::App.run"])
find_referencing_symbols(target="App.run")
history(target="py:src/app.py::App.run")
read(operation="slice", path="src/app.py", lines=[[40, 80]])
capabilities()
```

A wrong `operation` is answered with the valid list, and a parameter foreign
to the selected operation is refused with a message naming what that
operation accepts and requires.

This v2 surface is the only one. 0.11.0 removed the twelve per-question v1
tools (`repo_map`, `expand_symbols`, and the rest): `--surface v1` and
`--surface both` stop the server at start-up with exit 2, and `--surface v2`
is accepted. The mapping from each v1 tool to its v2 call is in
`agentless-mcp guide --section the-two-surfaces`.

### What the server runs

The MCP server does not apply patches or execute repository commands. Git is
the only program the read surface spawns, and every git call the package makes
goes through one bounded runner: a fixed configuration prefix on the argv, an
environment with git's redirection variables stripped, `LC_ALL=C`, a deadline,
and a cap on the output read from the pipe.

The prefix holds the settings a repository's own configuration could otherwise
use to name a program. `core.fsmonitor`, `diff.external` and `core.pager` are
fixed on the argv, and `log.showSignature=false` keeps a repository-local
`gpg.program` from running on every `git log`, which is what the receipt's
churn count and `history` read. The CLI's patch sandbox passes `--no-textconv`
on its diff, the one control git offers over a repository-named textconv
driver.

Two `GIT_` variables survive the environment scrub: `GIT_CONFIG_GLOBAL` and
`GIT_CONFIG_NOSYSTEM`. They move where git reads configuration from rather
than which repository it reads, so a caller that pins git's configuration
reaches these calls too. Every other `GIT_` variable is removed, because that
family can redirect git away from the repository the call named.

Each tool answers on a worker thread rather than on the event loop. One slow
call -- a cold map, a 30-second `git log -L` -- therefore does not stall the
answers to the others, and calls on one connection can overlap.

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
      "mcp__agentless__capabilities",
      "mcp__agentless__history"
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
`claude mcp add ... agentless ...` line at the top of this section.

### Eager tool schemas

Claude Code can defer an MCP server's tools: they arrive as bare names, and
the schema is fetched before the tool can be called. Five of the six tools
publish an `alwaysLoad` hint that asks a deferring client to hold their
schemas from the first turn, so the gate below redirects an agent that already
knows what each tool answers. Measured, the hint costs nothing: the deferred
and eager arms were indistinguishable on every localization metric, and
deferral spent a round trip fetching the schemas anyway. The gate is the
load-bearing part either way. `history` publishes no hint by design: it
answers why rather than where, so a deferring client fetches its schema only
when an agent asks.

### Structural-first gate (recommended hooks)

Prose asks for the structural pass. A hook enforces it. Two hook commands,
`agentless-mcp hook gate-check` and `agentless-mcp hook gate-mark`, deny
repository-wide or directory-wide `Grep`, every `Glob`, and any `Bash`
command that parses as a tree search, until the session has made a
localizing Agentless call.
`orient(map|path)`, `symbols(find|overview|expand|explain)`, `read(slice)` and
`find_referencing_symbols` unlock the session; diagnostics, `read(dir)`,
`symbols(locate)`, `history`, and the shape listings `orient(communities|cycles|diagram|health)`
do not. `read(slice)` unlocks on the same rule that allows an exact-file `Grep`:
naming a file and a line range is the localization. A directory listing is how
you look for a file, not evidence that you found one. `Grep` scoped to one
existing file remains available before unlock because the caller has already
localized that search. The client
runs the mark hook only after a call that succeeds. A call that returns an
error result does not unlock the session: a refused call, or a call that gives
no data for any item that it names.
The constraint is an order, not a ban: once the gate opens, broad search keeps
the one job a symbol map cannot do, which is string literals, error messages,
config keys, and fixtures.

**Search routed through the shell is covered too.** The check hook reads
`Bash` commands rather than trusting the tool name, and denies only what
parses cleanly as a tree search: `rg` with no path operand or a directory
operand, `grep` with a recursive flag, and `find` with `-name`/`-path`.
Everything else passes, including a pipe filter over another command's output
(`git log | grep fix`) and a search scoped to one existing file. Shell text
cannot be parsed in general, so this half fails open: `git grep`, `xargs`,
subshells, and variable command names all pass unexamined, and this
heuristic has not been benchmarked the way the tool-name half has.

This is the recommended install rather than an optional extra, and the reason
is measured: on SWE-Explore-Bench (n=60 issue-localization tasks, Sonnet,
agentless-mcp 0.6.1), an arm restricted to the agentless tools plus `Read`
beat a free-choice arm on all six localization metrics, every 95% confidence
interval excluding 0. Read that as evidence for the ordering, not as a
prediction for your repository: the measured arm removed the native search
tools, and this gate only defers them.

Publishing `history` costs nothing detectable on the same instrument, measured
twice. Two paired 60-instance runs put the hooked arm on 0.7.3 against the
hooked arm on 0.8.0, differing only in the server build. No acceptance metric
moved significantly against the release in either run, and `nDCG@100` favoured
it in both. A same-configuration replicate measured the run-to-run noise floor
those deltas are read against. The full comparison,
its guardrails, and the schema-policy arms are in
[`docs/analysis/benchmark-methodology.md`](docs/analysis/benchmark-methodology.md).

#### Claude Code

Install the gate by adding one hooks block to the Claude Code settings.

1. Merge the block in `contrib/hooks/settings-example.json` into
   `~/.claude/settings.json` for every project, or into the repository's
   `.claude/settings.json` for one. JSON allows no comments, so the file
   carries placeholder paths and this section carries the explanation.
2. Replace each `/ABSOLUTE/PATH/TO/agentless-mcp` placeholder with the
   absolute path of the installed `agentless-mcp`. For a uv tool install,
   `uv tool dir --bin` prints the directory that holds it. A bare
   `agentless-mcp` works only when it is on the `PATH` that Claude Code gives
   its hooks.
3. Start a new session. Claude Code reads `settings.json` at session start.

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Grep|Glob|Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/home/you/.local/bin/agentless-mcp hook gate-check"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "mcp__agentless__.*",
        "hooks": [
          {
            "type": "command",
            "command": "/home/you/.local/bin/agentless-mcp hook gate-mark"
          }
        ]
      }
    ]
  }
}
```

Verify the hooks outside a session first. Feed each one a fake hook payload
on stdin and read the exit code:

```sh
rm -rf /tmp/agentless_gate

# No structural call yet: exit 2, and the denial text goes to stderr.
echo '{"session_id":"probe","tool_name":"Grep","tool_input":{"pattern":"needle"}}' \
  | agentless-mcp hook gate-check; echo "exit=$?"

# One localizing structural call marks the session.
echo '{"session_id":"probe","tool_name":"mcp__agentless__orient","tool_input":{"operation":"map"}}' \
  | agentless-mcp hook gate-mark

# Now the same Grep is allowed: exit 0, no output.
echo '{"session_id":"probe","tool_name":"Grep","tool_input":{"pattern":"needle"}}' \
  | agentless-mcp hook gate-check; echo "exit=$?"

rm -rf /tmp/agentless_gate
```

The first call prints the denial text and reports `exit=2`. The third call
prints nothing and reports `exit=0`. Inside a session, a denied `Grep` shows
as a blocked tool call, and the model receives the same denial text as an
instruction to call `orient` first. The unlock marker lives under
`/tmp/agentless_gate/`.

**Both hooks fail open.** Malformed stdin, a missing session id, an
unwritable `/tmp`, or any internal error exits 0 and allows the call: the
failure mode to expect is a `Grep` that runs early, never a session that
cannot search. Set `AGENTLESS_GATE_LOG` to a file path to append one JSONL
line per decision and confirm the gate fired. Remove the gate by deleting
the two hook blocks from `settings.json` and the directory
`/tmp/agentless_gate`. Nothing else on disk changes.

The hooks trust `/tmp/agentless_gate` only when it is a directory, not a
symlink, that the user owns and that no group or other user can write.
Otherwise they count no marker, and the check allows every call. They create
the directory with mode `0700`. The Python hook scripts of earlier releases
create it with mode `0775` under `umask 0002`. After those scripts have run,
the check allows broad search until the directory is removed.

#### Codex CLI

The two hook commands are unchanged between clients. Codex uses the same
`PreToolUse` and `PostToolUse` events, the same `session_id`, `tool_name` and
`tool_input` payload fields, and the same exit-2-blocks-with-stderr contract.
Three things differ, and all three are configuration.

1. **Name the MCP server `agentless`.** The server name is the tool-name
   prefix: an entry named `agentless-mcp` produces `mcp__agentless-mcp__orient`,
   which the mark hook's `mcp__agentless__.*` matcher never sees, and the
   unlock half goes dormant.

   ```toml
   [mcp_servers.agentless]
   command = "/ABSOLUTE/PATH/TO/agentless-mcp-server"
   args = ["--root", "/ABSOLUTE/PATH/TO/repo"]
   ```

   Pass every repository you work in as a `--root`. A server given one root is
   blind in every other repository, and answers there look like an empty map
   rather than a misconfiguration.

2. **Write `~/.codex/hooks.json`** (or the same block inline under `[hooks]`
   in `~/.codex/config.toml`; a repository-local `.codex/hooks.json` also
   works). The `PreToolUse` matcher gains `exec_command`, Codex's unified-exec
   tool name.

   ```json
   {
     "hooks": {
       "PreToolUse": [
         { "matcher": "Grep|Glob|Bash|exec_command",
           "hooks": [{ "type": "command",
                       "command": "/ABSOLUTE/PATH/TO/agentless-mcp hook gate-check" }] }
       ],
       "PostToolUse": [
         { "matcher": "mcp__agentless__.*",
           "hooks": [{ "type": "command",
                       "command": "/ABSOLUTE/PATH/TO/agentless-mcp hook gate-mark" }] }
       ]
     }
   }
   ```

3. **Trust the hooks once.** Codex does not run a non-managed hook until it has
   been reviewed. Open the TUI and run `/hooks` to inspect and trust both. Until
   then they are skipped silently -- no error, no log line, and the gate simply
   does not fire.

Codex searches through its shell tool rather than a `Grep` tool, so there the
shell half of the gate does the work. The `anthropic/alwaysLoad` hint does not
port -- Codex always defers MCP schemas -- so Codex runs deferred schemas plus
the gate, which measured indistinguishable from eager on every metric.

### Erosion gate (per-edit hook)

`agentless-mcp hook erosion` checks each Claude Code `Write` and `Edit` call
before the call lands. It computes the whole text the call would leave on
disk, runs lint on that text in its own process, and reports what the change
makes worse. The checks are `complexity_delta`, `contract_drift`, and
`clone_delta`. `complexity_delta` reports a function above the complexity
threshold that gets more complex, a function that crosses the threshold, and
a new function above it. `contract_drift` reports a silent
default or a swallowed error in a module that `boundary_globs` names.
`clone_delta` reports a block of 3 or more source lines and 40 or more tokens
that the change adds while the repository already has a copy of it, with
names and literals ignored.

**Modes.** Each check has one mode: `off`, `advisory`, or `block`.

- `advisory` adds the findings to the model's context. The call then goes
  through the normal permission flow. The hook never approves a call.
- `block` stops the call with exit 2 and returns the findings to the model.
  Blocking findings come first, then the advisory findings under their own
  label.
- `off` skips the check.

The repository's `erosion_gate` key in `.agentless-mcp.json` sets a mode per
check, for example `{"erosion_gate": {"complexity_delta": "block"}}`. A check
that the key leaves unset takes `AGENTLESS_EROSION_GATE` (`advisory` or
`block`). Without that variable, the check takes `advisory`. The variable sets
`clone_delta` too, so `AGENTLESS_EROSION_GATE=block` blocks on copies. To keep
`clone_delta` advisory while it is on trial, set
`{"erosion_gate": {"clone_delta": "advisory"}}`.
`AGENTLESS_EROSION_GATE=off` turns the gate off on the whole machine, and it
wins over every repository key.

| Variable | Effect |
|---|---|
| `AGENTLESS_EROSION_GATE` | `off` turns the gate off everywhere. `advisory` or `block` sets the default mode. |
| `AGENTLESS_EROSION_LOG` | A file path. The hook appends one JSON line per call to this file. In each record, `binary` is the path of the running `agentless-mcp`. |

The hook runs lint in its own process and ignores `AGENTLESS_EROSION_BIN`,
which only the Python hook script of earlier releases read.

In the hook form, `clone_delta` reads the clone blocks of every source file
outside test and fixture paths from the tag cache (`agentless-mcp index`, or
the MCP server's background index). It parses only the edited file and the
files that changed or were added since the index. Without a tag cache, the
check is off for that call: lint gives one `not_checked` row with the reason
`no_tag_cache`, and the hook logs `clone_delta` under `off_by_config`, not as
a gap. An edit of a test or fixture file gets no `clone_delta` row. For a
Write that appends a 4-line function to `src/readers/document_readers.py` in
a clone of mcp-local at `f17af96`, the whole hook process takes 25.4 ms with
a fresh cache and 14.5 ms with no cache. These are medians of 60 runs (p95
27.0 ms and 15.6 ms) of `scripts/hook_latency.py` on b158b3f.
`lint --since` and candidate runs build the index when there is no cache, and
they stop it after 10 s. The hook reads the cache that a server or an
`agentless-mcp index` of the same release writes. Beside a Python-build
server, a 0.11.0 hook gets the `no_tag_cache` row until a 0.11.0 server or
`agentless-mcp index` fills the cache.

Install the gate in Claude Code:

1. Merge the `Write|Edit` entry from `contrib/hooks/settings-example.json`
   into the `PreToolUse` list in `~/.claude/settings.json`. Replace the
   placeholder with the absolute path of the installed `agentless-mcp`, as
   for the search gate above.
2. Keep `"timeout": 15`. The hook stops its own work after 10 s, so the 15 s
   timeout is a second limit. A hook that times out does not block the call.
3. Set the variables in the `env` block of `settings.json`, for example
   `"env": {"AGENTLESS_EROSION_LOG": "/home/you/.claude/erosion-gate.jsonl"}`.
4. Start a new session.

```json
{
  "matcher": "Write|Edit",
  "hooks": [
    {
      "type": "command",
      "command": "/home/you/.local/bin/agentless-mcp hook erosion",
      "timeout": 15
    }
  ]
}
```

Check the hook outside a session first. A Write of a small new Python file in
a repository gives exit 0, no output, and a `clean` record:

```sh
printf '%s\n' '{"session_id":"probe","transcript_path":"/tmp/probe.jsonl","tool_use_id":"toolu_probe","tool_name":"Write","tool_input":{"file_path":"/path/to/repo/probe.py","content":"x = 1\n"}}' \
  | AGENTLESS_EROSION_LOG=/tmp/erosion-probe.jsonl agentless-mcp hook erosion
echo "exit=$?"
tail -1 /tmp/erosion-probe.jsonl
```

**The gate fails open.** A malformed payload, a lint error, a lint timeout, a
SIGTERM from Claude Code, and any internal error exit 0 and let the call
proceed. The log records each of them as `degraded` with the reason. A file
that is not a tier-1 source file, a file outside a git repository, and a
symlink that points out of the repository are `skipped`. The log never records
a degraded call as `clean`.

**The log.** Each record holds the session id, the transcript path, the
`tool_use_id`, the path, the mode per check, the outcome, every finding with
its numbers, the state that lint observed in the proposed text, and the
latency. The outcomes are `clean`, `advisory`, `blocked`, `degraded`, and
`skipped`.

**The report.** `agentless-mcp gate-report` reads the log and the transcript
of each record after the sessions end:

```sh
agentless-mcp gate-report ~/.claude/erosion-gate.jsonl
agentless-mcp gate-report ~/.claude/erosion-gate.jsonl --sample-out labels.csv
# Write correct or wrong in the label column of each row, then:
agentless-mcp gate-report ~/.claude/erosion-gate.jsonl --labels labels.csv
```

The report puts each finding in one class:

- **not applied**: the call failed, or the user or a hook rejected it.
- **unresolved**: the transcript is missing or has no result for the call.
- **unobserved**: no later call in the same session changed the same file.
- **acted on**: a later call brought the function back to its old complexity
  or under the threshold, removed the contract hit, or left the file with
  fewer copies of the block.
- **not acted on**: a later call changed the file, and the finding still holds.

It prints the effective false-positive rate (not acted on, over acted on plus
not acted on) as a diagnostic. `--sample-out` writes a seeded random sample of
up to 30 applied findings per check to a new CSV file, and it never overwrites
a file. `--labels` reads the filled file and computes the labeled precision.

**The trial.** Run the gate in `advisory` mode first. Set a check to `block` in
a repository only when the report says that the check qualifies: at least 50
applied findings, and a labeled precision of 90% or more. The log cannot show
the third condition. Confirm that the check's rules and message did not change
after its trial started.

### Navigation craft for the agent

The gate decides *when* the structural pass happens; the routing knowledge
behind it ships with the package. `agentless-mcp guide` prints the full agent
usage guide -- the canonical recipe, the escalation funnel, and per-tool
usage -- and `--section NAME` prints one section for pasting into `CLAUDE.md`
or a dispatch prompt. The rules that matter most:

- **Seed the map with what the task already names**: paths, module stems,
  symbol names, or a decorated spelling like a traceback frame. One exact
  path outweighs a name that matched twenty files. A seed that matches
  nothing comes back in `unresolved_seeds`; reseed rather than trusting an
  unfocused ranking.
- **Escalate one rung at a time and stop at the rung that answers**:
  `orient(map)` to locate, `symbols(overview)` for what a file declares,
  `symbols(expand)` for the few bodies that matter, `symbols(explain)` for
  one suspect symbol's wiring, `find_referencing_symbols` only when callers
  decide the answer (it is the expensive one), `read(slice)` last.
- **Weigh reference tiers**: `same-file` and `resolved-via-import` are
  bindings; `unique` and `name-only-ambiguous` are candidates to inspect.
- **Run `Grep` after the structural pass** for what a symbol map cannot
  hold: string literals, error messages, config keys, fixtures.
- **Read an empty or thin map as a report, not an absence**, and call
  `capabilities` to separate a repository that never parsed from one that
  declares nothing there.

## Supported languages

The bundled grammars support Bash, C, C++, C#, Go, HCL, Java, JavaScript, JSON,
Kotlin, Lua, PHP, Python, Ruby, Rust, Scala, SQL, Swift, TOML, TSX, TypeScript,
and YAML. Run `capabilities` to see the grammars available in the current
installation.

## License

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
