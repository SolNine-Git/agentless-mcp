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
format. `validate` normalizes them against HEAD, runs byte-identical resulting
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

### Eager tool schemas

Approval is not the only way a session loses the server. Claude
Code can also defer an MCP server's tools: they arrive as bare names, and the
schema is fetched before the tool can be called. A deferred tool is not a
callable tool, and `Grep` loads from the first turn.

These five tools therefore publish an `alwaysLoad` hint, which asks a
deferring client to hold all five schemas from the first turn. The reason is
what the hint buys at the moment the gate in the next section fires: the
denial redirects an agent that already knows these tools exist and what each
one answers, rather than one meeting them for the first time because it was
blocked. Loading a schema on demand makes the tool callable; loading it up
front makes the tool *considered*.

The obvious objection is that preloading five schemas costs context in every
session. Measured on the benchmark below, it does not: the deferred arm spent
*more* than the eager arm on cache-creation tokens, cache-read tokens, turns,
output tokens and wall time, though no interval excluded zero. Deferral does
not keep the schemas out of context; it makes the agent spend a round trip
fetching them first.

The gate remains the load-bearing part either way. Ordering is what earned the
measured gains; the hint only ensures the agent understands its options before
the ordering is enforced.

### Structural-first gate (recommended hooks)

Prose asks for the structural pass. A hook enforces it. Two scripts in
`contrib/hooks/` deny repository-wide or directory-wide `Grep`, every `Glob`,
and any `Bash` command that parses as a tree search, until the session has
made a localizing Agentless call.
`orient(map|path)`, `symbols(find|overview|expand|explain)`, `read(slice)` and
`find_referencing_symbols` unlock the session; diagnostics, `read(dir)`,
`symbols(locate)`, and the shape listings `orient(communities|cycles|diagram|health)`
do not. `read(slice)` unlocks on the same rule that allows an exact-file `Grep`:
naming a file and a line range is the localization. A directory listing is how
you look for a file, not evidence that you found one. The equivalent v1 tools unlock servers running the temporary
compatibility surface. `Grep` scoped to one existing file remains available
before unlock because the caller has already localized that search. The mark
hook reads the call and not its result, so an Agentless call that errored
still unlocks.
The constraint is an order, not a ban: once the gate opens, broad search keeps
the one job a symbol map cannot do, which is string literals, error messages,
config keys, and fixtures.

**Search routed through the shell is covered too.** A harness that instructs
the model to search with `grep`, `rg` or `find` inside `Bash` produces payloads
whose tool name is always `Bash`, so a matcher keyed on `Grep|Glob` never sees
them and the deny half goes dormant. The check hook therefore reads `Bash`
commands rather than trusting the tool name, and denies only what parses
cleanly as a tree search: `rg` with no path operand or a directory operand,
`grep` with a recursive flag, and `find` with `-name`/`-path`. Everything else
passes, including the common case of a pipe filter over another command's
output (`git log | grep fix`) and a search scoped to one existing file.

Shell text cannot be parsed in general, so this half of the gate is a weaker
guard than the tool-name half and is deliberately biased to fail open. `git
grep`, a search assembled by `xargs` or a subshell, and any command whose name
arrives through a variable all pass unexamined. The benchmark backing the
"recommended install" claim measured the tool-name matchers only; the shell
heuristic has a different error profile and has not been measured.

This is the recommended install rather than an optional extra. The server
assumes the ordering mechanism lives client-side: the schemas it asks a client
to preload say what the tools do, and the gate is what decides when they are
reached for.

The measured reason is a paired comparison on SWE-Explore-Bench, run against
agentless-mcp 0.6.1 with n=60 issue-localization tasks on Sonnet. An arm
restricted to the agentless tools plus `Read` beat an arm with every tool
available on all six metrics: precision +0.062, recall +0.041, F1 +0.040,
hit-region rate +0.052, WCC +0.043, and recall@100 +0.011, with every 95%
confidence interval excluding 0. Prompt steering alone did not produce that
ordering discipline in the free-choice arm. Read the numbers as evidence for
the ordering, not as a prediction for your repository: the measured arm removed
the native search tools, and this gate only defers them.

Both schema policies were then measured as their own arms, on the same 60
tasks with the same gate and prompt, differing only in whether the client
preloaded the schemas. They are indistinguishable on every metric except
`recall@100`, where deferral led by 0.017 -- one significant result among
roughly 22 tests, and the tools-only arm holds the highest `recall@100` of any
arm while being eager-loaded, so the exception does not track schema policy.
Eager loading is what ships; the gate is what earned the gains in either case.

How that comparison is run, which guardrails it carries and why, and what the
gated arm itself measured are in
[`docs/analysis/benchmark-methodology.md`](docs/analysis/benchmark-methodology.md).

#### Claude Code

Install the gate by copying the two scripts and adding one hooks block.

1. Copy `contrib/hooks/agentless_gate_check.py` and
   `contrib/hooks/agentless_gate_mark.py` to a stable path, for example
   `~/.claude/hooks/`.
2. Merge the block in `contrib/hooks/settings-example.json` into
   `~/.claude/settings.json` for every project, or into the repository's
   `.claude/settings.json` for one. JSON allows no comments, so the file
   carries placeholder paths and this section carries the explanation.
3. Replace each `/ABSOLUTE/PATH/TO/...` placeholder with the absolute path of
   the copied script. A relative path does not resolve.
4. Keep the `/usr/bin/env python3` prefix. Claude Code runs the `command`
   string as a shell command, not as an argv list, so the interpreter has to
   be named.
5. Start a new session. Claude Code reads `settings.json` at session start.

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Grep|Glob|Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/bin/env python3 /home/you/.claude/hooks/agentless_gate_check.py"
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
            "command": "/usr/bin/env python3 /home/you/.claude/hooks/agentless_gate_mark.py"
          }
        ]
      }
    ]
  }
}
```

Verify the scripts outside a session first. Feed each one a fake hook payload
on stdin and read the exit code:

```sh
rm -rf /tmp/agentless_gate

# No structural call yet: exit 2, and the denial text goes to stderr.
echo '{"session_id":"probe","tool_name":"Grep","tool_input":{"pattern":"needle"}}' \
  | python3 ~/.claude/hooks/agentless_gate_check.py; echo "exit=$?"

# One localizing structural call marks the session.
echo '{"session_id":"probe","tool_name":"mcp__agentless__orient","tool_input":{"operation":"map"}}' \
  | python3 ~/.claude/hooks/agentless_gate_mark.py

# Now the same Grep is allowed: exit 0, no output.
echo '{"session_id":"probe","tool_name":"Grep","tool_input":{"pattern":"needle"}}' \
  | python3 ~/.claude/hooks/agentless_gate_check.py; echo "exit=$?"

rm -rf /tmp/agentless_gate
```

The first call prints the denial text and reports `exit=2`. The third call
prints nothing and reports `exit=0`. Inside a session, a denied `Grep` shows
as a blocked tool call, and the model receives the same denial text as an
instruction to call `orient` first. The unlock marker is
`/tmp/agentless_gate/<sha256(session_id)>.json`; its receipt names the tool and
operation that unlocked broad search without placing the untrusted session id
in a filesystem path.

**Both hooks fail open.** Malformed stdin, a payload with no session id, an
unwritable `/tmp`, or any other internal error exits 0 and allows the call. A
gate that breaks an unrelated session is worse than a gate that misses one
call. The failure mode to expect is a `Grep` that runs early, never a session
that cannot search.

Set `AGENTLESS_GATE_LOG` to a file path to append one JSONL line per decision.
The variable is optional and unset by default. Use it to confirm that the gate
fired rather than assuming it did.

Remove the gate by deleting the `PreToolUse` and `PostToolUse` blocks from
`settings.json` and starting a new session. Delete `/tmp/agentless_gate` and
the copied scripts afterwards. Nothing else on disk changes.

#### Codex CLI

The two scripts are unchanged between clients. Codex uses the same
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
   args = ["--allow-client-roots", "--root", "/ABSOLUTE/PATH/TO/repo"]
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
                       "command": "/usr/bin/env python3 /ABSOLUTE/PATH/TO/agentless_gate_check.py" }] }
       ],
       "PostToolUse": [
         { "matcher": "mcp__agentless__.*",
           "hooks": [{ "type": "command",
                       "command": "/usr/bin/env python3 /ABSOLUTE/PATH/TO/agentless_gate_mark.py" }] }
       ]
     }
   }
   ```

3. **Trust the hooks once.** Codex does not run a non-managed hook until it has
   been reviewed. Open the TUI and run `/hooks` to inspect and trust both. Until
   then they are skipped silently -- no error, no log line, and the gate simply
   does not fire.

Codex searches by running `rg`, `grep` and `find` through its shell tool
rather than through a `Grep` tool, so on that client the shell half of the
gate does the work and the tool-name half rarely fires.

**Eager schema loading does not port.** The `anthropic/alwaysLoad` hint this
server sets is Anthropic-specific and does not appear in Codex; Codex's own
`tool_search_always_defer_mcp_tools` is fixed on and cannot be overridden. So
Codex runs deferred schemas plus the gate. That is the 0.6.2 configuration,
which measured indistinguishable from eager on every metric, so the cost is
a round trip rather than a capability.

### Navigation craft for the agent

The gate decides *when* the structural pass happens. It does not teach the
pass. This section is the routing knowledge behind it. Read it while
configuring an agent, or paste it into `CLAUDE.md` or a dispatch prompt.

**Seed the map with what the task already names.** `orient(operation="map",
focus=[...])` ranks every file by personalized PageRank over the reference
graph, and the seeds take the whole teleport mass, so the ranking flows
outward from what you name. A seed that resolved leads the ranked list, and
the files that reference it rank above the utilities it imports. A seed
resolves as a repository-relative path (`src/billing/invoice.py`), a path
suffix (`invoice.py`), a bare module stem (`invoice`), a qualified symbol
name (`Invoice.total`), or a bare symbol name (`quote`). Lift them from the
task: the file in the traceback, the class in the ticket, the function in
the error message. Each seed carries one vote, split across the files it
matched, so one exact path outweighs a name that matched twenty files. A
seed that matches nothing does not fail the call and does not disappear. It
comes back in `unresolved_seeds`, and as a `# note:` line above the map.
Read that note: the ranking under it is not focused the way you asked. The
usual cause is that the name is a parameter, an attribute or a DSL keyword
rather than a declared symbol, and `symbols(operation="find")` says which.

**Escalate one rung at a time, and stop at the rung that answers.** Each rung
costs more than the one above it.

1. `orient(operation="map")` answers *where this lives*. It returns ten ranked
   files by default however large the repository is: it localizes, it does not
   enumerate. Below the ranked files it may list the tests that exercise them,
   which the ranking itself does not surface, because a test file is held out
   of the ranking as a pure source of rank.
2. `symbols(operation="overview", paths=[...])` answers *what this file
   declares*. Signatures with the bodies elided, plus the stable-id pattern for
   that file. It is cheap, and it replaces reading the file.
3. `symbols(operation="expand", stable_ids=[...])` answers *what this code
   does*. Line-numbered bodies for the few symbols that matter. Batch the short
   ones; expand a long one on its own, because a batch over the output budget
   cuts the longest bodies to their leading lines and marks the cut.
4. `symbols(operation="explain", target=...)` is the one-call card for a single
   suspect symbol you have not seen: definition site, signature, linked
   rationale comments, fan-out and fan-in by evidence tier, and the file's
   imports. It replaces a `find` followed by a reference listing.
5. `find_referencing_symbols` answers *who calls this, and what breaks if it
   changes*. Pay for it when the answer depends on the callers -- a blast
   radius, an error path, a signature change -- not as a routine confirmation
   of something a map already showed. It is the expensive tool here, and
   fan-in across a large repository can take minutes.
6. `read(operation="slice")` is the last rung. Use explicit line ranges when no
   symbol boundary fits the region you need.

Weigh the reference rows rather than trusting them equally. `same-file` and
`resolved-via-import` are resolved bindings. `unique` means only that the
repository spells that name once, which is retrieval evidence and not a link
between the two files. `name-only-ambiguous` is a candidate to inspect.

**Run `Grep` after the structural pass, for what a symbol map cannot hold.**
These tools rank symbols, so a file that matches the task only as text never
enters a map, whatever you seed it with: string literals, error messages,
config keys, fixtures, templates, and the wiring in YAML or JSON. The two
passes are not alternatives. The structural pass finds the code and gives you
the exact names to search for; the text pass then finds everything about those
names that is not code. Going in the other order costs the map's ranking,
which is the part a text search cannot reproduce.

**Read an empty or thin map as a report, not as an absence.** When nothing
ranks, the map says which of three things happened: nothing in the repository
parsed into symbols, the file cap kept none of the ranked files, or the token
budget left room for none of the candidate symbols. Only the first is a
statement about the repository. When a view stops short, call `capabilities`:
it reports the server version, the loaded grammars and their warm state, the
cache generation, the configured and client-advertised roots, and the caps in
force, and it names the exact index command when the tag cache is absent. That
is what separates "this repository was never parsed" -- an unsupported
language, a cold cache, or a root that is not the one you meant -- from "this
repository genuinely declares nothing here". An agent that skips the check
reads the second when the first is true, and stops looking.

## Supported languages

The bundled grammars support Bash, C, C++, C#, Go, HCL, Java, JavaScript, JSON,
Kotlin, Lua, PHP, Python, Ruby, Rust, Scala, SQL, Swift, TOML, TSX, TypeScript,
and YAML. Run `capabilities` to see the grammars available in the current
installation.

## License

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
