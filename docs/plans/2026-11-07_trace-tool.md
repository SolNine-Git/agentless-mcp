# Design: the trace tool, the search tool, and hooks that supply grounding

Status: design for the owner's review, 2026-11-07, `feat/trace-tool` at 2e8f807. It follows the owner's decisions
of 2026-11-07 (brief TDES) and two later ones: no migration period, and `agentless-mcp hook install` behind `make
install-hooks`. Each example answer is frozen-CLI 0.11.0rc1 output on this repository, set in the new grammar.

## 1. The problem and the evidence

- **The schema is large.** tools/list publishes 7 tools, 17 call forms and 40 parameters: 10,526 bytes, measured as
  `len(json.dumps(tools, separators=(",", ":")))` of the `tools` array that the frozen 0.11.0rc1 binary answers over
  stdio (`exec -a agentless-mcp-server <binary> --root <clone>`, one root, no port).
- **Denial costs more than it returns.** 300 conf2 tasks against the native arm
  (`/var/tmp/claude-rust-port/perfpage/data/session_cost.json`, `files_found.json`): the gated arm spent +115.8%
  context and +53.3% priced cost for +0.0054 WCC [-0.0053, +0.0166]. The forced arm (Grep and Glob removed) spent
  +46.0% context and +22.8% cost for +0.0019 WCC [-0.0099, +0.014]. Neither gain is distinct from zero.
- **Supply pays.** The prompt-map hook (240 IRP tasks) raised WCC by +0.0119 [+0.0043, +0.0199] with 5.9% less
  context; the placebo moved it by -0.0009 [-0.0068, +0.0053] (`README.md`, "Prompt map").
- **The tool choice is split** (`/var/tmp/claude-rust-port/perfpage/data/tool_choice.json`). Native tools win the
  literal (16% of the gated agent's Grep calls), the pattern (14%), the file name and the behavior search. `symbols
  find` is at parity with Grep (3.74 against 4.10 new correct files per 1,000 tokens); a map call gives 0.76 to
  0.87. `find_referencing_symbols` got 0 calls in 1,200 sessions; `symbols expand` got the most (109 calls).
- **A subagent gets no map.** The prompt-map hook does not fire for a subagent's first prompt (verified
  2026-11-07). Over 1,194 recorded briefs, a brief hook would emit on 784 (66%), and 44% of the files it adds are
  already named in the brief.
- **Most real searches and many reads go through Bash** (`/var/tmp/claude-rust-port/replay/report.md`). In the
  owner's main sessions: 0.47 Grep calls against 3.17 Bash searches per session, and Bash reads of 2,277 Claude
  tokens against Read's 4,762. So `search` competes with Bash grep, and the card hook must serve single-file Bash
  reads: a Read-only hook misses 1.80 reads per main session and 10.99 per subagent session.
- **A cleaner pays on search answers, not on reads.** Lossless rules remove 17.7% of the native Grep answer tokens
  and 0.07% of the Read answer tokens. Root-relative paths in the call inputs explain the forced arm's output
  saving: 167.5 Claude tokens per session derived, 168.5 measured.
- **Supply is the cheapest policy.** Replayed on the 300 native sessions at a 450-token card: supply 4.77% of the
  session price with no denial, the shipped gate 10.6%, an unlock budget 20.6% to 38.3%, a denying file ledger
  27.1%.
- **A card finds as much as the native first answer, with fewer tokens** (`/var/tmp/claude-rust-port/trace/report.md`,
  299 tasks). For the names of the agent's first Grep, traced in one call, a card with the callers and tests sections
  (L2) names 37.2% of the gold files in 420 tokens. The native first answer names 37.9% in 531. Adding the callees
  (L3) gives 39.4% in 524. One name traced alone gives only 29.5% at L3, so the batch form matters.
- **Most first lookups do not resolve to one definition.** Of the agent's first names, 39% have several definitions,
  23% have none and 31% have one. The candidates and miss answers carry much of the value, and a miss's usage rows
  have the highest yield of any section.
- **The composed card is correct.** 181,256 checks of 108,472 card lines against the source answers found 0
  disagreements, and a mutation test of the checker caught 825 of 828 planted errors.

Facts from the Claude Code documentation (`https://code.claude.com/docs/en/<page>`, read 2026-11-07):

| Fact | Page, section |
|---|---|
| PreToolUse and PostToolUse fire for a subagent's tool calls; the input carries `agent_id` and `agent_type`. | `hooks`, "Hook locations", "Common input fields" |
| SubagentStart `additionalContext` is added "before its first prompt". Its input has `agent_id`, `agent_type` and no prompt. It cannot block. | `hooks`, "SubagentStart", "SubagentStart input" |
| UserPromptSubmit fires on typed prompts, scheduled tasks, background subagent reports and session messages; not on a subagent's first prompt. | `hooks`, "UserPromptSubmit" |
| `updatedInput` replaces the whole input; "With no `permissionDecision`, auto mode can deny the changed call". The command-hook exemption does not name Agent. | `hooks`, "PreToolUse decision control", "PreToolUse `updatedInput` in auto mode" |
| Agent needs no permission. Auto mode evaluates "the delegated task description" before a subagent starts. | `tools-reference`, "Agent tool behavior"; `permission-modes`, "How auto mode handles subagents" |
| Deny and ask rules apply whatever a hook returns. In plan mode a hook "allow" does not approve a call that is not read-only. | `permissions`, "Precedence among rules, hooks, and mods", "Hook approvals in plan mode" |
| `updatedToolOutput` "must match the tool's output shape"; for a built-in tool a mismatch "is ignored". The Read shape is not documented. | `hooks`, "PostToolUse decision control" |
| PostToolUse `additionalContext` goes "alongside the tool result" (order not documented), capped at 10,000 characters. | `hooks`, "PostToolUse decision control", "JSON output" |
| Edit needs a read in the conversation for older models; `cat`, `sed -n`, `grep` or `rg` of one file counts. The page names no MCP tool, so an MCP read does not count. | `tools-reference`, "Edit tool behavior" |
| On macOS, Linux and WSL, Claude Code leaves Glob and Grep out of the default tool set, and Claude searches with `find` and `grep` through Bash. | `tools-reference`, "Glob tool behavior" |
| Exit 2 blocks only PreToolUse among the events used here. SessionStart matchers: `startup`, `resume`, `clear`, `compact`, `fork`. A handler that two settings files define runs once. | `hooks`, "Exit code 2 behavior per event", "SessionStart", "Hook handler fields" |

Consequence: native Read stays, because an agent that edits needs it. The first-read card uses `additionalContext`,
not `updatedToolOutput`, because the Read shape is unknown and a wrong shape is dropped with no error.

## 2. The new surface

tools/list publishes `trace` and `search`: read-only, idempotent, closed-world, with the `anthropic/alwaysLoad`
hint. Draft size: 2,697 bytes for both by the measure of section 1, plus 176 bytes per tool for `repo_root` with
several roots (`/var/tmp/claude-rust-port/tdes/scripts/schema_size.py`; derived until the build serves it).

### 2.1 `trace`

| Parameter | Type | Default | Limits |
|---|---|---|---|
| `target` | string, or array of strings | required | 1 to 8 items |
| `more` | enum: `candidates`, `callers`, `callees`, `tests`, `similar`, `history`, `body`, `declares`, `imports`, `importers`, `cycles`, `tree` | none | one value |
| `repo_root` | string | the launch root | published only with more than one root |

Description (Strict): "Show what a code target is and how it connects. target is a name, a repository-relative
path, path:line or a stable id. A name with more than one definition gives the candidates. A function, method or
class gives its signature, callers by evidence tier, callees, tests, similar functions and last change. A file
gives its declarations, imports, importers, tests and import cycles. A folder gives its tree. Each section shows how
many rows it hides, and more shows one section in full; more=body gives the numbered body. A list of targets gives
one card per target in one call." Parameters: `target` "A name, a path, path:line or a stable id. A list traces
each one." `more` "One section to show in full."

**Resolution order.** (1) A stable id. (2) `path:line`: the innermost symbol that spans the line
(`graph::owners::enclosing_symbol`), else the file card with a note. (3) A repository-relative path: a file gives
the file card, a folder the folder card; a leading `/`, a blob URL or a traceback frame reads as in the map's focus
rule. (4) A qualified name, then a bare name, matched exactly as `explain` matches. One match gives its card;
several give the candidates card. (5) No match gives the miss answer.

**Answer grammar.** The receipt line, then one card per target in the order given, separated by a blank line. With
more than one target, each card starts with `target: <as given>`; a repeat prints `same card as target <n>`.

```
header   = "[" id "] @" span "  " kind NL "  " signature                   (symbol card)
         | path "  " language "  " n " lines  ids: " pattern               (file card)
         | path "/  " n " files, " n " lines"                              (folder card)
         | name ": " n " definitions. Trace one by its id."                (candidates card) ;
section  = name ": " counts [ "  (more=" name ")" ] NL { "  " row NL } ;
row      = path "  " tier "  " site { ", " site }  |  id " @" span "  " kind ;
site     = qualname " @" line | "(module level) @" line ;
counts   = n " bound" [ "; " n " name-only in " n " files hidden" ] ;
```

Binding tiers (`same-file`, `imported`) print first, in full up to the row cap; `unique` and `name-only` print as
counts. A position is `@line`; verbatim text uses the `N|` gutter (only `more=body`). Every `path @line` row reads
back as `trace("path:line")`. An empty section prints `<name>: none`.

**Row caps** (the trace report's level L4). A symbol card shows 3 rows each of callers (binding tiers, tests left
out), tests, callees and similar, the counts of every tier, the "not resolved" line and the last change. A file card
shows 8 declarations and 3 rows each of importers, tests and imports, and its cycle membership. A candidates card
shows 16 definitions. A miss shows 10 usage files. A folder card shows depth 1 and 60 entries; the trace report did
not measure folders apart, and `more=tree` gives the rest.

The trace report's lowest expected cost is L3, which stops before `similar` and `last change`. L4 costs +45 [-23,
+106] priced tokens more, a difference not distinct from zero, at 636 tokens against 524: inside the owner's
ceiling of 1.5 times. L4 is the default because its `similar` section serves duplicate avoidance, which the
benchmark cannot see. In sessions with 16 or more requests after the card, the knee moves down one level (the
report's sensitivity table). The in-situ log and the trail run can move the default.

**Symbol card**, `trace("rs:crates/hook/src/prompt_map.rs::answer")`, from `explain`, `refs` and `history`:

```
[rs:crates/hook/src/prompt_map.rs::answer] @117-135  function
  fn answer( hook: &Hook<'_>, services: &Services, stdin: &mut dyn Read, stdout...
callers: 3 bound; 253 name-only in 35 files hidden  (more=callers)
  crates/hook/src/prompt_map.rs  same-file  prompt_map @73
  crates/hook/src/prompt_map/tests.rs  imported  (module level) @12, Rig.run_with @233
callees: 9 bound; 3 name-only hidden  (more=callees)
  crates/hook/src/prompt_map.rs  same-file  Facts @108, Hook @79, INTERNAL @49, decide @156, entry @268, written @137
  crates/app/src/env.rs  imported  Services @113
  crates/hook/src/search/log.rs  imported  append @43, now @29
tests: 1
  crates/hook/src/prompt_map/tests.rs  imported  Rig.run_with @233
similar: 321 candidates, ranked by name-only callers; hidden  (more=similar)
last change: de91a250 2026-11-05 Add the prompt-map hook: the map's first files for a first prompt  (more=history)
```

`similar` ranks by shared bound callers. With no shared bound caller, it prints only the count and its tier: today
`refs --shared-callers` ranks `py:packaging/check_wheel.py::HttpClient.answer` first, through 18 callers of other
`answer` functions. `last change` runs `git log -L` for one commit under a deadline; past it the line reads
`last change: not read in <n> s  (more=history)`.

**Candidates card**, `trace("answer")`: `answer: 14 definitions. Trace one by its id.`, one row per definition up
to the cap, such as `  rs:crates/app/src/find.rs::answer @118-138  function`, then `  ... 11 more  (more=candidates)`.

**File card**, `trace("crates/hook/src/prompt_map.rs")`, from `map`, the `explain` imports and `cycles`:

```
crates/hook/src/prompt_map.rs  rust  304 lines  ids: rs:crates/hook/src/prompt_map.rs::<QualifiedName>
declares: 42 (14 functions, 2 structs, 1 type alias, 25 constants); 25 constants hidden  (more=declares)
  pub(crate) type MapFn @25, fn prompt_map @58, struct Hook @79, fn marker_dir @90, fn in_process @97, struct Facts @108, fn answer @117, fn written @137, fn decide @156,
  fn repository_root @184, fn claim @194, fn is_plain_file @209, fn mapped @213, fn context @248, fn entry @268, fn optional @292, fn texts @296
imports: 9
  crates/app/src/lib.rs, crates/git/src/lib.rs, crates/io/src/lib.rs, crates/json/src/lib.rs, crates/kernel/src/lib.rs, crates/hook/src/search.rs, crates/hook/src/search/log.rs, crates/hook/src/text.rs, crates/hook/src/worker.rs
importers: 2
  crates/hook/src/lib.rs @34, crates/hook/src/prompt_map/tests.rs @12
tests: crates/hook/src/prompt_map/tests.rs
cycles: in 1 import cycle of 27 files  (more=cycles)
  crates/hook/src/call.rs -> crates/hook/src/lib.rs -> crates/hook/src/call.rs
last change: de91a25 2026-11-05 Add the prompt-map hook: the map's first files for a first prompt
```

A file's last change is `git log -1 -- <path>`, which is cheap. A file card has no `body`: Read reads a file.

**Folder card**, `trace("crates/hook/src/search")`: `crates/hook/src/search/  7 files, 1185 lines`, then one row
per entry, such as `  check.rs  212 lines  33 symbols` and `  tests.rs  553 lines  51 symbols`.

**Misses and notes** are ordinary answers (`isError: false`). A miss lists up to 10 files that use the name, from the
usage files that `find-symbol` reports. With none, a bounded literal scan (2.2) lists at most 3 files. Either way
the next call is not blind:

| Case | Answer text (real data) |
|---|---|
| No match | `no symbol, file or folder matches zz_no_such. It occurs as text in 1 file: crates/mcp/fixtures/calls.json @4. search(pattern="zz_no_such") lists every match.` |
| No symbol spans the line | `no symbol in crates/prompts/src/generated/catalog.rs spans line 837; this is the file card.` and the file card |
| Line past the end | the existing `target_no_line` text with the true line count |
| File not parsed | `crates/checks/fixtures/seeded.json is not parsed: 1308730 bytes exceeds the per-file cap of 1000000 bytes. Read it or search it.` |
| No grammar | `<path>: no grammar reads <suffix> files. Read it or search it.` |
| Section absent | `more=callers does not apply to a file card. File sections: declares, imports, importers, tests, cycles.` |
| Unknown path | `no file or folder <path> in the repository. Paths that end the same way: <up to 3>.` |

### 2.2 `search`

| Parameter | Type | Default | Limits |
|---|---|---|---|
| `pattern` | string | none | a valid regular expression |
| `files` | string | all files | glob: `*`, `**`, `{a,b}` |
| `path` | string | the root | a repository-relative file or folder |
| `ignore_case` | boolean | false | |
| `context` | integer | 0 | 0 to 10 |
| `limit` | integer | 100 (open value) | 1 to 1000 |
| `repo_root` | string | the launch root | published only with more than one root |

Description (Strict): "Search the text of the repository files, or list files by name. pattern is a regular
expression in ripgrep syntax. Without pattern, the answer lists the files that files matches. Paths are relative to
the root. Matches are grouped by file and by enclosing symbol, with its stable id: give the id to trace. Lock,
vendored and minified files show as count lines. Use search for literals, messages, config keys and
patterns. Use trace for a name that you know." Parameters: `pattern` "A regular expression, matched line by line.
Omit it to list files." `files` "A glob on repository-relative paths, such as **/*.rs or src/{a,b}/**." `path` "A
repository-relative file or folder. Default: the root." `ignore_case` "Ignore letter case." `context` "Lines shown
before and after each match. Default 0." `limit` "Maximum match lines or file rows. Default 100."

**Engine.** The file list is the walk that `tree` and `index` use (gitignore and the walk caps apply). The pattern
uses the `regex` crate: the ripgrep dialect, linear time. It is the one new direct dependency (a `Policy-Change:
crates/dependencies.toml` commit that the owner approves). `regex` 1.13.1 is already in `Cargo.lock` through
`tree-sitter` and `tiktoken-rs`, so the direct dependency adds no crate to the build. `git grep` is rejected: another regex dialect, PCRE is
not in every git build, and a root need not be a git repository. The glob is the boundary glob dialect of
`crates/checks/src/globs.rs`, moved to `crates/text` in a pure move, plus a new function that expands `{a,b}` first.
The enclosing symbol of a match comes from `graph::owners::enclosing_symbol` over the tag cache.

**Answer grammar.**

```
answer     = summary NL { file_group } { noise_line } [ rest_line ] ;
summary    = "search /" pattern "/" [ " in " path ] [ " files=" glob ] ": " n " matches in " n " files" ;
file_group = path NL { [ "  " qualname "  [" id "] @" span NL ] { "    " line "| " text NL } } ;
noise_line = "+ " n " matches in " n " " class " files: " path " (" n ")" { ", " path " (" n ")" } ". path=<file> shows them." ;
rest_line  = "... " n " of " n " match lines shown; " n " more files: " path " (" n ")" { ", " path " (" n ")" } ;
listing    = "search files=" glob [ " in " path ] ": " n " files" NL { path "  " n " lines" [ "  " n " symbols" ] NL } ;
```

Source files print first in path order, then other files. A match outside every symbol has no symbol line. A match
line whose text repeats in 3 or more places prints once, with every location. A noise class (lock, vendored,
minified) prints as a count line unless `path` names a file in it. A line is cut only in a minified file, and the cut
keeps its number: `text...[+1834 chars]`. Every other match line prints in full with its own line number.

The replay report sets these rules. Grouping by file and collapsing repeats removed 17.7% of the Grep answer tokens
and lost nothing that the agent used later. Three candidate rules lost data, so the tool does not apply them:
- Hiding generated files hid gold files: `rpc/flipt.pb.go`, `sdk/go/evaluation.sdk.gen.go`,
  `lib/matplotlib/pyplot.py` and `doc/help/settings.asciidoc`.
- Cutting long lines in code hid identifiers that the agent used later, in 21 of 31 changed answers.
- Line numbers on every 10th line only hid line numbers that the agent used later, in 535 of 545 answers.

**Content example**, `search(pattern="agentless_prompt_map")` (matches from `rg -n`, symbols from `explain`):

```
search /agentless_prompt_map/: 4 matches in 4 files
crates/hook/src/prompt_map.rs
  MARKER_DIR  [rs:crates/hook/src/prompt_map.rs::MARKER_DIR] @27
    27| const MARKER_DIR: &str = "agentless_prompt_map";
crates/hook/src/prompt_map/tests.rs
  a_first_prompt_that_names_a_file_gets_the_map_files_and_exits_0  [rs:crates/hook/src/prompt_map/tests.rs::a_first_prompt_that_names_a_file_gets_the_map_files_and_exits_0] @290-320
    316|         .join("agentless_prompt_map")
CHANGELOG.md
    25|   session (a marker in `$TMPDIR/agentless_prompt_map/`, with the search gate's
README.md
    616|   session id in `$TMPDIR/agentless_prompt_map/` (`/tmp/agentless_prompt_map/`
```

Other forms: `search(pattern="serde_json")` ends with `+ 25 matches in 1 lock files: Cargo.lock (25). path=<file>
shows them.` `search(files="crates/hook/src/search/*.rs")` gives `search files=crates/hook/src/search/*.rs: 7
files` and one row per file in the folder card's row form, such as `crates/hook/src/search/check.rs  212 lines  33
symbols`. The line and symbol counts come from the tag cache, so the agent can choose what to read without a call; a
file with no grammar prints no symbol count. No match is ordinary: `search /zz_no_such_text/: 0 matches in 1110 files searched.`

**Refusals** (`isError: true`): `pattern is not a valid regular expression: unclosed group at column 4. To match
the text, pass pattern="foo\("`, and `search needs pattern, files or both.` A `path` outside the root, unknown
parameters, wrong types and out-of-range values get the existing walk, `argument_*` and `accepts_*` messages. The
roots refusals, the deadline answer and the token ceiling apply to both tools unchanged.

## 3. The old surface: replacement in one release

The owner is the only user, so "contracts evolve additively" does not apply. One release replaces the 7 tools,
with no legacy option.

| Old call | New call |
|---|---|
| `orient(map)` | `trace` on a name or path; the prompt-map and subagent-map hooks supply the ranked files |
| `symbols(find)`, `symbols(explain)`, `symbols(locate)` | `trace(name)`, `trace(id)`, `trace("path:line")` |
| `symbols(overview)`, `read(dir)`, `symbols(expand)` | `trace(path)`; `trace(id, more="body")` with up to 8 ids |
| `find_referencing_symbols`, `history`, `graph(cycles)` | `more="callers"` (`shared_callers` is `more="similar"`); `more="history"`; a file card's `cycles` |
| `graph(path, communities, diagram, health)`, `capabilities` | CLI only; a miss or note names what is missing |
| `read(slice)` | native Read |

`--surface` refuses every value at start-up: "--surface is no longer served: this release publishes trace and
search only. Drop --surface." This text replaces the `surface_v1_deprecated` row and the v1 refusal.

Every CLI command stays, from `map` to `gate-report`, including `communities`, `health`, `diagram` and `html`: the
release gate's surface diff and the prompt-map hook use them. New CLI commands: `trace` and `search` (the tool
answers, for the goldens), and `hook install`, `hook status` and `hook uninstall`.

CHANGELOG entry, under "Changed": "**The MCP server publishes two tools, `trace` and `search`, in place of `orient`,
`symbols`, `read`, `graph`, `history`, `capabilities` and `find_referencing_symbols`.** tools/list falls from
<measured> to <measured> bytes with one root (cl100k <n> to <n>, by this section's method). `trace` returns one card
per target; `search` does the work of Grep and Glob with root-relative paths, matches grouped by file and enclosing
symbol, and noise as count lines. The CLI keeps every command. `--surface` is refused at start-up. `gate-check` and
`gate-mark` are retired; `hook install` removes their entries." Each new hook gets an "Added" entry (section 7).

## 4. The hooks

Each hook is a subcommand of the installed binary, `<absolute path>/agentless-mcp hook <name>`. Each hook reads
one JSON payload on stdin, never denies a call, never writes stderr, exits 0, and appends one line to
`AGENTLESS_GATE_LOG` when it is set (`hook`, `event`, `reason`, `session_id`, `agent_id`, `ms`). The settings
`timeout` is 3 times the hook's deadline. Deadlines marked "open" are set by the rule in section 9.

| Hook | Event, matcher | Input | Output | Deadline | Status |
|---|---|---|---|---|---|
| `read-card` | PostToolUse, `Read|Bash` | `session_id`, `agent_id`, `tool_input.file_path`, or `tool_input.command` for Bash | `additionalContext`: the file card | open, 1 to 5 s | new |
| `ledger-mark` | PostToolUse, `mcp__<server>__trace` | `session_id`, `agent_id`, `tool_input.target` | none | 2 s | new, replaces `gate-mark` |
| `brief-record` | PreToolUse, `Agent` | `session_id`, `tool_input.prompt`, `tool_input.subagent_type` | none, no decision | 1 s | new |
| `subagent-map` | SubagentStart, all types | `session_id`, `agent_id`, `agent_type`, `cwd` | `additionalContext`: one sentence | 5 s | new |
| `prompt-map` | UserPromptSubmit | `session_id`, `prompt`, `cwd` | `additionalContext`: one sentence | 5 s | keep |
| `erosion` | PreToolUse, `Write|Edit` | the edit | advisory text, or exit 2 in block mode | as today | keep |
| `ledger-reset` | SessionStart, `compact` | `session_id` | none | 1 s | new |
| `gate-check`, `gate-mark` | PreToolUse `Grep|Glob|Bash`; PostToolUse `mcp__agentless__.*` | | | | retire |

**The ledger** records which files a context holds a card for. A context is the main conversation or one subagent
(`agent_id`): a card in the parent's context is not in a subagent's context. Location: `$TMPDIR/agentless_ledger/`
(`/tmp/agentless_ledger/` when `TMPDIR` is unset or empty), by the prompt-map marker rule and its trust rule (a
private directory that the user owns). Layout: `<sha256(session_id)>/<"main" or sha256(agent_id)>/<sha256(canonical
path, NUL, sha256(content))>`, one empty file per entry, made by `io::write_new_file` (exclusive create). The first
claim wins, so parallel hooks stay safe. The key holds the content hash, so an edit makes the card stale and the
next Read gets a new card. Lifetime: the session. The OS temp cleaner removes old entries, and `ledger-reset`
removes the main context's entries at compaction. Cap: 2,000 entries per context, then silent with `ledger_full`.

**`read-card`.** On a Read of a source file (a code grammar; JSON, YAML, TOML, HCL and SQL do not count), or on a
Bash command that reads one source file, the hook hashes the file on disk and claims its ledger entry. A Bash read is
one of the read-before-edit views (`cat`, `nl`, `bat`, `head`, `tail`, `sed -n 'X,Yp'`, `grep` or `rg` on one file,
with no pipe or redirect), found by the shell parser that `gate-check` used. A held entry means silent. A new entry renders the file card of
2.1 in the hook's process (tag cache, `worker::run` under the deadline) and prints
`{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"File card (agentless-mcp), first read of
<path> in this context:\n<file card>"}}`. The repository is the git top level of the file, not of `cwd`. The claim
comes before the render, so a deadline uses the one try (the prompt-map rule). The card is cut to the card cap
(open value, below the documented 10,000 characters). Silent cases: not source, outside git, held entry, deadline,
render failure, malformed payload, untrusted storage. `tool_response` is not read: its shape is not documented.

**`ledger-mark`.** After a `trace` call, the hook resolves each `tool_input.target` with the same app function as
the server, and claims an entry for the file of each symbol card and file card. It reads the documented
`tool_input`, not `tool_response`. A later Read of that file gets no card. The matcher uses the server name that
`hook install --server-name` gives (default `agentless`).

**`brief-record` and `subagent-map`.** SubagentStart is the documented point to add context to a subagent, but its
input has no brief. `brief-record` (PreToolUse, Agent; skips the `fork` type, which inherits the parent's context)
writes the brief's seeds (prompt-map seed rules) and the files it names, not the brief, to
`<ledger>/<sha256(session_id)>/briefs/<type>-<ns time>`. It returns no decision and no `updatedInput`: no permission
effect in any mode. `subagent-map` takes the one pending record of its `agent_type` younger than 60 s. Two or more
pending records of one type are ambiguous: it removes them and stays silent (`ambiguous_pairing`). A resumed
subagent or a teammate message finds no record. It maps the seeds, drops the files the brief names, and takes the
first 2 of the first 5 remaining files (new template `subagent_map_context`). For a brief that names
`crates/hook/src/prompt_map.rs` and `claim` (ranking from `map --granularity file`): `Repository map
(agentless-mcp): for claim in this brief, the files that rank first and that the brief does not name are
crates/graph/src/owners.rs and crates/app/src/lib.rs.` A limit shows: `owners.rs` declares another `claim`.

**`prompt-map`** stays: it is measured, and the subagent map reuses its seed rules. **`erosion`** stays: it is
advisory by default, and `block` is the owner's opt-in per check. **`ledger-reset`** removes `<session>/main/` on
SessionStart `compact`, because compaction removes the cards from the context; a subagent's entries stay.

**`gate-check`, retired.** It denies Grep, Glob and shell search until a localizing call: the owner decided against
denial, and the gated arm cost +53.3% for a WCC gain not distinct from zero. `gate_deny`,
`crates/hook/src/search/{check,stat,marker}.rs`, the `/tmp/agentless_gate` marker and the `localizes` field of the
surface table go. `shell.rs` stays, because `read-card` uses its parser to find a single-file Bash read. **`gate-mark`, retired.** It marked a localizing call for `gate-check`; `ledger-mark` records
files instead. `hook gate-check` and `hook gate-mark` stay accepted for one release and exit 0 with no output, so a
stale entry does not break a session; `hook status` reports them as retired.

**Rejected:** rewriting the brief with `updatedInput`. With no decision, auto mode can deny the changed call; with
"allow", the hook approves calls it does not own; with "ask", every subagent start prompts. **Rejected:** replacing
the Read result with `updatedToolOutput`: the shape is not documented, and a wrong shape is dropped silently.

## 5. Installation and staleness

A Python wheel has no install step: the installer copies files and runs no package code, so `uv tool install` alone
cannot write the hooks. A Claude Code plugin can carry an MCP server and hooks (`plugins-reference`, "Standard
layout"), but it is rejected as the main route: its hooks keep the old version's path until `/reload-plugins`
(`plugins/loading`, "When auto-update runs"), auto-update is off for other marketplaces by default, and no plugin
field pins an external binary version (`plugins-reference`, "dependencies").

**Commands.** The logic lives in the binary (`crates/hook/src/install/`) with Rust tests.

- `agentless-mcp hook install [--scope user|project|local] [--repo PATH] [--server-name NAME] [--dry-run]`. `user`
  (default) writes `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`; `project` writes
  `<repo>/.claude/settings.json`; `local` writes `<repo>/.claude/settings.local.json`. `--dry-run` prints the
  change and writes nothing.
- `agentless-mcp hook status [--repo PATH]`: one line per expected entry; exit 0 when current, 1 when an entry is
  missing, stale or retired.
- `agentless-mcp hook uninstall [--scope ...]`: removes the agentless entries only.
- `make install-hooks`, in a new root `Makefile`: `$(AGENTLESS_MCP) hook install`, where
  `AGENTLESS_MCP ?= $(shell uv tool dir --bin)/agentless-mcp` (the installed build, never the build under test).

**The entry table.** `app::hooks` holds one table (hook, event, matcher, timeout) and the pure comparison. `hook
install` writes the table, and `hook status` and the server compare against it. Both live in `app` (layer 5),
because `hook` and `mcp` are both layer 7 and a crate depends only on lower layers.
`contrib/hooks/settings-example.json` becomes a golden of `hook install --dry-run` on an empty file.

**Idempotence and safety rules.**

1. An agentless entry is a `command` handler whose command is `<path> hook <name>`, where the file name of `<path>`
   is `agentless-mcp` and `<name>` is a current or retired hook. No other handler is touched.
2. Install removes every agentless entry and each matcher group that becomes empty, keeps a group that still holds
   another handler, then appends one group per table row. The command path is the canonical path of the running
   binary, which a uv tool install keeps across upgrades.
3. Key order and every other key stay (2-space JSON). A result byte-equal to the file is not written (`no change`).
   A file that does not parse is refused (exit 1, the parse error, no write); a missing file is created.
4. The write is atomic (temporary file in the same folder, then rename), keeps the mode, and writes a symlink at its
   target. The first install keeps `settings.json.agentless-backup`. A file that changed between the read and the
   rename gets one retry, then a refusal. Report: `added <n>, replaced <n>, removed <n> retired (<names>), kept <n>
   other handlers; wrote <path>`.

**Staleness.** The hook code cannot go stale: each hook runs the installed binary, which an upgrade replaces. The
entries can: a release adds a row or changes a matcher, or the path no longer holds the binary. The server reads the
`hooks` key (no other key) of the user file and of each root's project and local files at start-up and when an mtime
changes. An entry is current when its row matches the table and its path resolves to the server's own binary. A
state that is not current adds one note under the receipt of each client session's first answer: `// note:
hooks: 2 of 7 agentless hook entries are missing (read-card, ledger-reset) and 1 is retired (gate-check). Run:
agentless-mcp hook install`. Other forms: `hooks run /x/agentless-mcp; this server runs /y/agentless-mcp`, and
`hooks: <path> is not readable: <error>; hook state unknown`.

**README steps** (P5): install, register the server, `hook install` (or `make install-hooks`), `hook status`.

## 6. The formatting standard: checklist per new answer

1. Every caller-facing string is a row in `crates/prompts/data/*.json`: descriptions, labels, notes, misses,
   refusals, hook sentences, the install report and the server note. `cargo xtask generate` writes the Rust, and
   the staleness test and the template counts (`crates/prompts/src/tests.rs`, `xtask/src/generate.rs`) pass.
2. Each new template has a row in `crates/prompts/fixtures/render.json` with sample values and the rendered text.
3. Each card kind, `more` section, miss, refusal and search form has a byte-exact golden (text, and JSON where the
   CLI has `--json`) under `tests/characterization/goldens/{trace,search}/` on `repo_py`, `repo_ts`, `repo_go` and
   `repo_nested`, and each hook output a golden of its stdout under `goldens/hooks/`. Each new or changed golden has
   its cl100k counts in `crates/app/fixtures/read/tokens.json`.
4. `crates/app/fixtures/surface/surface.json` holds the new tools/list byte-exact; CHANGELOG states its bytes and
   cl100k count by the CHANGELOG method.
5. Paths are root-relative; a position is `@line`; verbatim text has the `N|` gutter; ids are in brackets; each
   hidden row is counted with its `more=` name; the receipt line is first.
6. Descriptions are ASD-STE100 Strict. `isError` is true only for a refusal; a miss names the next useful call.
7. An answer over the token ceiling carries the envelope's truncation line. Each `public-api.txt` is current.

## 7. The build plan

One builder per phase, with a disjoint file set. A phase starts when the API it builds on is merged.

| Phase | Owns | Builds on | Tests and goldens |
|---|---|---|---|
| P0, PM | the open values; the `regex` approval | the reports | none |
| P1, core | `crates/app/src/{trace,search,hooks}*`, the glob move (`crates/checks/src/globs.rs` to `crates/text/src/glob.rs`, pure move first), `crates/{app,text,checks}/src/lib.rs`, `crates/app/Cargo.toml`, `crates/dependencies.toml`, `crates/{app,text,checks}/public-api.txt` | 2e8f807 | resolution order; each card kind as typed values; the enclosing symbol per match; brace expansion; noise classes; the entry table and the comparison over fixture settings files |
| P2, text | `crates/prompts/**`, `crates/render/src/{trace,search}*`, `crates/render/public-api.txt`, `xtask/src/generate.rs`, `tests/characterization/goldens/{trace,search}/`, `crates/app/fixtures/read/tokens.json` | P1 | checklist items 1, 2, 3, 5, 6 and 7, for every string in sections 2, 4 and 5 |
| P3, server and CLI | `crates/mcp/**`, `crates/app/src/surface*`, `crates/app/fixtures/surface/`, `crates/cli/**`, `crates/agentless/src/server*`, `crates/agentless/tests/server_door.rs` | P1, P2 | tools/list golden; both tools dispatched; the 7 tools and the operation tables removed; `--surface` refused; the hook note (first answer per session, mtime reload); CLI `trace` and `search` equal the tool text |
| P4, hooks and install | `crates/hook/**`, `crates/agentless/src/main.rs`, `crates/agentless/tests/dispatch.rs`, `contrib/hooks/`, `Makefile`, `tests/characterization/goldens/hooks/` | P1, P2 | per hook: payload fixtures and every silent case; the ledger under two processes (one claim); pairing with one, two and no pending records; install run twice is byte-equal; foreign handlers and mixed groups kept; invalid JSON refused; symlink target; atomic write; retired entries removed; `settings-example.json` equals the dry-run golden |
| P5, docs and page | `README.md`, `crates/app/data/agent-guide.md`, `CHANGELOG.md`, `xtask/src/raw_outputs.rs` | P3, P4 | the guide test; the `raw-outputs` test of section 8 |
| P6, release gate | none | P5 | the four steps below |
| P1b, follow-ups | `crates/git/src/**`, `crates/git/public-api.txt`, `crates/app/src/trace/file.rs` | P1 | the file card's last change (`git log -1 -- <path>`), which P1 could not build because the git crate has no function for it; runs beside P2 |

No phase and no test writes the owner's real settings file: the install tests use fixture files, and the first real
`hook install` runs only on the owner's word. Only P3 and P4, and P1b and P2, run at the same time, on disjoint
sets; a later phase can edit a merged phase's file (P3 edits `crates/app/public-api.txt`).
`scripts/gate-selftest.sh "$gate"` runs in P2 and P5, which change `xtask/`.

P1 as built (03b109a) leaves three items for later phases. (1) A symbol card runs 3 scans (resolution, refs,
explain) and a file card runs 3 (resolution, explain, cycles), because the explain and refs internals are private.
P3 measures the card latency with the CLI; a refactor that shares one scan waits for that measurement. (2) The
last-change thread lets a late `git log -L` run to git's own 30 s limit. (3) A file's imports come from `explain`
on one of its symbols, so a file with no symbol in the name graph, such as an `__init__.py` with only imports, gets
no import rows (`Linked::NoAnchor`).

**Release regression gate** (`AGENTS.md`; `docs/analysis/benchmark-methodology.md`, "The release regression gate"):

1. The suites and pre-commit. Prediction: the goldens of the removed MCP answers go, the new ones come, and no
   `map`, `refs`, `explain` or `cycles` golden changes.
2. The Loc-Bench retrieval tier from a frozen copy, `ranked_files` byte-diffed. Prediction: 0 of 150 differ. Free.
3. The surface diff. Prediction: CLI `map`, `cycles`, `refs --json` and `explain` are byte-equal between the builds.
   Added: every row of each `trace` card on the sampled targets agrees with its CLI source (TRC check). Free.
4. The exposure check: the new surface has no recorded exposure. Each paid run needs the owner's approval:
   - **PAID, approval needed:** a probe of 4 to 6 `claude -p` sessions that launch parallel subagents of one type,
     to verify the `brief-record` and `subagent-map` pairing and the `agent_id` key of `read-card`.
   - **PAID, approval needed:** the trail run, native against the new surface with the hooks, solo and sequential,
     n set by the margin (about 192 instances for 0.019 WCC). It measures an agent that stops before a follow-up.

**Testing before a release.** The owner requires a large amount of testing before a release (2026-11-07). The
release gate above is the minimum. The full plan, cheapest first:

1. Each phase: unit tests, byte-exact goldens for every answer form, and every gate of `AGENTS.md`. No existing
   golden changes unless the change is predicted first.
2. Differential test, free: the Rust `trace` builds the cards for the 299 bench targets of the trace report, and a
   script compares them row by row with the Python prototype (`/var/tmp/claude-rust-port/trace/scripts/tracecard.py`).
   Each difference is explained or fixed.
3. Replay of recorded calls, free: each recorded call to an old tool maps to its `trace` or `search` call. Every fact
   of the old answer is in the new answer or one `more=` call away.
4. The release gate above, free.
5. A review at the end of each phase, sized to the change. The PM asks the owner before a full panel review.
6. The hooks probe in a sandbox, paid and small, with a separate `CLAUDE_CONFIG_DIR`, never the owner's settings.
7. A trial in the owner's environment, on the owner's word: `hook install` on the owner's settings, normal work for
   some days, then the hook log read as counts only.
8. The trail run, paid, on the owner's approval.
9. A release candidate first (0.12.0rc1). The final release only on the owner's word.

## 8. The artifact of raw outputs

The page shows, for each tool form and each hook, the input and the raw output as the model sees it: the MCP text
content with its receipt line, and for a hook the stdout JSON and the `additionalContext` string that Claude Code
wraps in a system reminder, each with its cl100k count from `tokens.json`. `cargo xtask raw-outputs --out <dir>`
builds the page from `tests/characterization/goldens/{trace,search,hooks}/` and the request file beside each golden.
The page holds no text that a golden does not hold, apart from labels; its footer prints the commit and the sha256
of each golden. A test asserts that every golden in those folders is on the page and that the page has no other
content source. The suites compare each golden with the code, so the page cannot show an output that the code at
that commit does not give. The PM publishes the page with the Artifact tool after P5 and after each golden change.

## 9. Open values and risks

| Open value | Rule that sets it |
|---|---|
| `read-card` card cap | set: 1,000 characters. The L4 file card is 366 characters median, 766 p95 and 847 at most over 24 sampled files; the row caps bound the card, and the cap guards against long paths and signatures |
| Search: default `limit` and the rule that classes a file as minified | answer sizes on the Loc-Bench checkouts; the replay report set the other rules (2.2) |
| Listing row counts (lines, symbols) | keep them if the listing's median size grows by at most 50% over a path-only listing on the Loc-Bench checkouts |
| Ledger key | settled: (session, context, path, content hash). Every subagent transcript carries its parent's session id, so the context (`agent_id`) is part of the key. The hash costs +0.15 to +0.18 points of session price over the path alone |
| `read-card` deadline and timeout | 3 times the p95 card latency on the Loc-Bench checkouts, from 1 s to 5 s; timeout 3 times the deadline |
| `last change` deadline | 3 times the p95 of `git log -L` for one commit on the Loc-Bench checkouts, at most 2 s |
| Ambiguous pairing share | the share of the 1,194 recorded briefs launched beside another brief of the same type in one message |
| Ranked files read for the subagent dedupe | 5, until the replay gives the depth that keeps 2 new files on 90% of briefs |
| Cycles line for Rust intra-crate cycles | the yield of the cycles section on Rust file cards; the trace report gives no per-section yield by language, so P1 measures it on the Loc-Bench checkouts |
| Release version | the owner |

Risks:

- **Adoption.** No call is denied, so the agent can keep Bash grep over `search`; supply acts only on reads. Only
  the paid trail run measures the choice. The place of the card next to the Read result is not documented.
- **Pairing.** SubagentStart has no brief and no tool-use id: the per-type pairing is an inference until the probe.
- **Name collisions.** A bare name ranks other definitions of the name (the `owners.rs` example), and `similar`
  ranks through name-only callers when no bound caller is shared.
- **Latency and races.** `git log -L` can take 30 s on a large repository (one commit, under a deadline). Claude
  Code also writes `settings.json`; the atomic write and the retry lower the race. The server reads only `hooks`.
- **A count can be silently short.** At `--limit` 175 or more, `explain` drops the whole 467-row ambiguous tier of
  teleport `Forwarder.authenticate`, and only the top-level `truncated` field says so (trace report). `trace` takes
  each count from the complete list, never from a capped payload. A P1 test pins this on a fixture with a tier
  larger than the payload ceiling.
- **Removed MCP views.** `path`, `health`, `communities`, `diagram` and `capabilities` had 0 calls in the
  2026-09-27 hooked arm; the CLI keeps them. `/tmp/agentless_gate` stays on disk, and nothing reads it.
