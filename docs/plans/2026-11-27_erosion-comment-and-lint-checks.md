# Specification: `comment_delta` and `linter_delta` in the erosion gate

Branch: `feat/erosion-comment-and-lint-checks`, from `main` at 16b89d9. Epic: PB-84. Lint ticket: PB-85
(sub-task PB-86). Status: validated (CONCERNS, folded in below), ready to build.

Line numbers are from 16b89d9. Lines marked "(map)" come from the agent maps of 2026-11-27 and were
not all re-read. Re-trace each one before you edit.

## Problem Statement

The comment rules live outside agentless, in three Python files under `~/.claude/hooks`:
`comment_rules.py` (the rules), `comment_gate.py` (PreToolUse on Write and Edit, blocks), and
`comment_gate_scan.py` (PostToolUse on Bash, Stop and SubagentStop). The files hold their own lexers
for Python, JS/TS and Rust, and they deploy and drift apart from agentless. The agentless erosion gate
already compares the old and the new text of each edit and reports only new findings. The erosion plan
copied that rule from `comment_rules.py` (`docs/plans/2026-10-09_erosion-checks.md:60`).

The lint gate is the other half of PB-84. A per-repo `quality-gate.sh` runs `uv run ruff check` on the
whole file after each Claude Code edit, and it blocks on findings the edit did not add (PB-85).

This plan moves the comment rules into agentless as a gated check, `comment_delta`, adds `linter_delta`
(PB-85) with a ruff row, and adds one agentless hook, `erosion-scan`, for the writes that skip Write
and Edit.

## Locked Decisions (owner, 2026-11-26 and 2026-11-27)

1. `comment_delta` blocks by default. The other gated checks keep their advisory default.
2. A file outside a git repository gets no comment check (`crates/hook/src/destination.rs:52`).
3. The shell-write half moves into agentless as `erosion-scan`. The three Python files retire.
4. Data languages (JSON, YAML, TOML, HCL, SQL) are out of `comment_delta`, with a declared gap.
5. Per-language rows ship in phases. A follow-up ticket under PB-84 adds the rest.
6. `linter_delta` lives inside agentless (owner, 2026-11-26, in PB-86). It stays advisory until its trial
   shows under 10% effective false positives. Whole-project checks stay out of the hook.
7. When a finding blocks, the output ends with fix lines and no mode line, for every check in block
   mode. Advisory output keeps the mode line.
8. `AGENTLESS_EROSION_SCAN_IGNORE` lists the repository roots that `erosion-scan` never scans.
9. `linter_delta` runs in every git repository whose file has a ruff config above it.
10. agentless gains no new file extensions in this work. All local `.pyi` and `.jsx` files are in the
    Loc-Bench checkouts, and the owner's repositories hold none.

## Research Summary

### Key Findings

- **The erosion pipeline.** A Write or Edit enters `hook::erosion` (`crates/hook/src/lib.rs:78`). The
  hook resolves the file and the git root, loads the modes, builds the proposed text, runs `app::lint`
  with `LintForm::Proposed` on a worker thread, and maps the rows to a verdict
  (`crates/hook/src/gate.rs:76-211`, `crates/hook/src/verdict.rs:37-135`). The call deadline is 10 s
  (`crates/hook/src/call.rs:17`). The install timeout is 15 s, from a 5 s `Hook::Erosion.deadline()`
  (`crates/app/src/hooks.rs:125`, map). `AGENTLESS_EROSION_GATE=off` turns off the whole hook
  (`crates/hook/src/lib.rs:88`).
- **Tier 1 is 12 languages.** bash, c, cpp, go, java, javascript, lua, python, ruby, rust, tsx and
  typescript (`crates/syntax/src/table_tests.rs:166-188`). Only these have a `spec()`, and
  `checks::pair_language` and the hook's `require_tier1` keep only these
  (`crates/checks/src/pair.rs:189-192`, `crates/hook/src/destination.rs:30-39`). C#, Kotlin, PHP, Scala
  and Swift are tier 2 and never reach a pair check.
- **The check tables.** `PairCheck::ALL` has 6 checks and `CHECKS` has 13
  (`crates/app/src/registry.rs:64-78,173-180`). `GatedCheck::ALL` has 3 (`crates/io/src/config.rs:102-113`).
  `Modes` is `[GateMode; 3]` (`crates/hook/src/modes.rs:13`). The hook links a `GatedCheck` to a
  `PairCheck` by name only (`crates/hook/src/gate.rs:133`, map). Two tests pin the template count, 418
  (`crates/prompts/src/tests.rs:106`, `xtask/src/generate.rs:227`).
- **One default for all checks.** A mode comes from the repository's `erosion_gate` entry, then from
  `AGENTLESS_EROSION_GATE`, then from advisory (`crates/hook/src/modes.rs:39-65`). The owner's settings
  do not set the variable, and no local repo sets `erosion_gate`.
- **The block text names the off switch.** `findings_text` ends every output with `mode_line`
  (`crates/hook/src/verdict.rs:163-203`). `MAX_LISTED = 15` caps the findings, not the lines
  (`verdict.rs:13`). The existing finding messages already say how to fix each finding (for example
  `complexity_grew` in `crates/prompts/data/messages.json`).
- **A gap never reaches the agent.** A `not_checked` row is off by config or a gap, and a gap reaches
  only the log and the `degraded` outcome (`crates/hook/src/verdict.rs:37-82,105-135`). `off_by_config`
  matches on the check and the gap together (`verdict.rs:71-82`).
- **gate-report sees three checks.** `Tracked` and `Observed` cover only complexity, contract and clone
  (`crates/hook/src/report/log.rs:46-65,85,232,252-254`, `crates/hook/src/report/classify.rs:135-160`,
  map). A finding of any other check classifies as unobserved.
- **No check runs a subprocess, and `io::run` cannot feed stdin.** Only git runs a child, with an
  `io::Spec` timeout (`crates/git/src/run.rs:193`). `io::Spec` gives the child a null stdin
  (`crates/io/src/runner.rs:24,162`).
- **Pair checks have registry flags.** A pair check that is not `opt_in` runs in the CLI default, in
  `lint --since`, and in the MCP lint tool (`crates/app/src/registry.rs:99-108,249`,
  `crates/app/src/lint/candidates/selection.rs:48`, map). `RunInputs` holds no root, no on-disk path and
  no time budget (`crates/app/src/pair_runs.rs:25-34`, map).
- **The hook table has one event per hook.** `Event` has 5 values and no Stop or SubagentStop
  (`crates/app/src/hooks.rs:73-89`). `entries()` emits one row per hook (`hooks.rs:186-196`). `compare`,
  `state` and `run::replied` key on the hook alone (map).
- **`lint --since REV` pairs base and working-tree text.** `revision::compared` lists committed, staged,
  unstaged and untracked changes against a commit (`crates/app/src/revision.rs:36-62`). It has no file
  cap and does not choose a merge base. `trace diff:` does, with `git merge-base`
  (`crates/app/src/trace/diff.rs:131-149`, map).
- **Session state is exclusive-create files.** A second process gets `Held`, and one context holds at
  most 2,000 entries (`crates/hook/src/state.rs:23-47`, `crates/hook/src/ledger.rs:20,31-85`, map).
- **No agentless code detects a shell write.** `search/shell.rs` recognizes reads for read-card and
  rejects any command with a redirect (`crates/hook/src/search/shell.rs:9-24`, map).
- **The comment rules.** `comment_rules.py` holds 17 rules at 16 finding sites, plus 3 exemptions
  (table in D3). The key holds the rule kind and the exact text, never the line. Old and new keys compare
  as multisets per file (`comment_rules.py:511-519`). Python counts only a bare `pub` as public in Rust
  (`comment_rules.py:424`). `facts` counts every visibility modifier (`crates/facts/src/extract/rust_items.rs:52-56`).
- **ruff.** ruff 0.16.3 reads stdin with `--stdin-filename` and writes JSON with the code, message,
  location, fix and URL of each finding. Every finding reports severity `error`. A syntax error gives one
  `invalid-syntax` finding per error, and a config error exits 2 with empty stdout. Without
  `--force-exclude`, ruff lints a path that the repo's config excludes (validator experiment). A
  `pyproject.toml` with only `[tool.ruff.lint]` counts as a config (validator experiment). The median of
  5 runs on athenaeum's 621-line `src/config.py` is 8.7 ms for `.venv/bin/ruff` and 30.6 ms for
  `uv run ruff`. Local venvs hold ruff 0.15.16, 0.16.3, 0.17.5 and 0.17.6. No ruff is on `PATH`.
- **oxlint has no stdin mode.** Its CLI takes paths only and finds its config relative to each file
  (oxc.rs, "Command-line Interface", read 2026-11-27). No local repo uses oxlint.
- **OMP replays Stop and SubagentStop**, and shows and logs SubagentStop feedback only
  (`~/.omp/agent/extensions/claude-hooks.ts:620-631`). OMP also reads each project's
  `.claude/settings.json` (`claude-hooks.ts:284-300`), so PB-85's claim that `quality-gate.sh` does not
  run in OMP is doubtful.
- **When a static check may block.** Google turns a check into a build error only when it is actionable,
  has no effective false positives, and covers correctness rather than style. Simple team-owned presubmit
  checks, such as formatting, also block. (Software Engineering at Google, ch. 20, "Compiler
  Integration", "Presubmits", "Tricorder".)

### Approach Decision

Both checks are opt-in pair checks inside agentless, driven by per-language tables, like
`contract_drift`. A test requires each language file in `crates/syntax/languages/` to have a row or a
declared gap. `comment_delta` is a pure function over the tree-sitter parse of the old and the new text.
`linter_delta` splits in two: the application layer runs the linter through `io::run` with stdin, and a
pure function in `crates/checks` diffs the parsed findings. `erosion-scan` runs the checks in block mode
over the files that a shell command changed, against the merge base.

### Trade-offs Accepted

- `comment_delta` blocks on style, which Google's compiler rule would not allow. The owner chose it. The
  rules are mechanical and serve one owner, like a team presubmit, and gate-report measures each block.
- Files outside git lose the comment check. `.pyi`, `.jsx`, `.mts` and `.cts` files lose it too, and
  no owner repository holds any of them today.
- The tree-sitter parse replaces three hand lexers. On text that does not parse, tree-sitter can report
  comments that Python's tokenizer stopped before. The parity run names each difference.
- A repo can turn `comment_delta` off with its `erosion_gate` key. Decision 7 keeps that switch out of
  the block text.
- JS/TS lint and all tier-2 languages move to the follow-up ticket.
- `erosion-scan` blocks do not reach gate-report in this work. They go to their own log.

## Design

### D1. A built-in default mode for each gated check (U0)

Give each `GatedCheck` a built-in default in `crates/io/src/config.rs`. The order of precedence becomes:

1. the repository's `erosion_gate` entry for the check,
2. `AGENTLESS_EROSION_GATE`, when it names `advisory` or `block`, for every check (its meaning today),
3. the check's built-in default.

`default_mode` returns `Option<GateMode>` and keeps its warning for a value that names no mode
(`crates/hook/src/modes.rs:52-65`). `Modes` holds `[GateMode; GatedCheck::ALL.len()]`. `off` still turns
off the whole hook. U0 changes no behavior: the three existing checks keep `advisory` as their built-in
default. Each new `GatedCheck` entry lands in the unit that adds its pair check (U1, U3). U0 adds a test
in `crates/hook` that every `GatedCheck` resolves to a `PairCheck` through `PairCheck::from_name`.

### D2. Block text (U0)

A blocked output has this layout:

```
Erosion gate blocked this edit:
<path>:<line>: <message>            (blocking findings first)
Advisory findings (these do not block):
<path>:<line>: <message>
...and <n> more findings            (at most 15 findings above this line)
<fix block of each check that blocks, in GatedCheck order>
```

There is no mode line. The fix block of `complexity_delta`, `contract_drift` and `clone_delta` is one
shared line, printed once: "Change the code as each blocking finding says, then retry the edit." The
fix block of `comment_delta` (U1) is the four rule lines of `comment_gate.py:42-48`, then "Delete the
flagged text, then retry the edit. Do not reword it." A block can reach 24 lines. Each fix line is a
message in `crates/prompts/data/messages.json` with a row in `crates/prompts/fixtures/render.json`.
Advisory output is unchanged.

### D3. `comment_delta` (U1, U2)

**Placement.** `crates/checks/src/comments.rs` and `comments/tests.rs`, exported as `comment_delta`.
The rows go in `crates/prompts/data/comment_rules.json`, served by a new generator module beside
`xtask/src/generate/rules.rs` and a reader beside `crates/prompts/src/rule.rs`. `crates/checks/Cargo.toml`
adds the approved `regex` crate. The check reads Rust doc markers (`outer_doc_comment_marker`,
`inner_doc_comment_marker`) as node kinds through `syntax`, so `crates/checks` gains no `facts` dependency.

**Registry row.** `opt_in`, so it runs in the hook, in `erosion-scan`, and with `lint --only`, but not in
the CLI or MCP default. It keeps every path: the Python gate exempts no test or fixture path.
Severity `warning`. `GatedCheck::CommentDelta` with built-in default `block` lands in U1.

**The rules (parity with `comment_rules.py`).** For each comment, the first matching branch wins:
suppression, then directive (exempt), then banner, then the language's doc rules, then the run rules.

| Lang | Rule | Key | Message | Source |
|---|---|---|---|---|
| Python | suppression (`noqa`, `nosec`, `type: ignore`, pylint/pyright/mypy/ruff disable or ignore) | `("suppress", comment)` | `lint suppression: {comment}` | `:21-23,154` |
| Python | banner (`# ====`, `# --- x ---`) | `("banner", comment)` | `banner comment` | `:19,158` |
| Python | own-line `#` run over 2 lines (a banner, a directive or a comment after code ends the run) | `("run", texts...)` | `{n}-line comment; why-comments run 1-2 lines` | `:123-143,160` |
| Python | docstring on a `_` function, dunders included | `("private-doc", name)` | `docstring on non-public function {name}` | `:169-178` |
| Python | `Args/Arguments/Parameters/Returns/Yields/Raises:` in a public function's docstring | `("section", name)` | `Args/Returns/Raises section in docstring of {name}` | `:24,179-182` |
| JS/TS | suppression (`eslint-disable`, `@ts-ignore`, `@ts-nocheck`, `@ts-expect-error`, `prettier-ignore`, `biome-ignore`) | `("suppress", head)` | `lint suppression: {head}` | `:28-30,244` |
| JS/TS | banner | `("banner", head)` | `banner comment` | `:26,248` |
| JS/TS | `@param`, `@returns`, `@throws` (and `@arg`, `@argument`, `@return`) in `/** */`, normalized, sorted | `("section", tags...)` | `JSDoc {tags} tags; the typed signature carries them` | `:31-32,250-254` |
| JS/TS | plain `/* */` over 2 lines | `("run", text)` | `{n}-line comment; ...` | `:255-256` |
| JS/TS | own-line `//` run over 2 lines | `("run", texts...)` | `{n}-line comment; ...` | `:257-258` |
| Rust | suppression attribute (`allow(`, `expect(`, `rustfmt::skip`, `cfg_attr(... allow/expect(`) | `("suppress", head)` | `lint suppression: {head}` | `:35-37,471-481` |
| Rust | doc comment on a trait-impl item | `("private-doc", name)` | `doc comment on trait-impl item {name}; overrides carry no docs` | `:417-418` |
| Rust | doc comment on a non-public item (bare `pub` only; trait and enum members take the parent's visibility; `macro_rules!` needs `#[macro_export]`) | `("private-doc", name)` | `doc comment on non-public item {name}` | `:419-425` |
| Rust | `# Arguments`/`# Args`/`# Parameters`/`# Params`/`# Returns`/`# Return` in a doc comment | `("section", name)` | `# Arguments/# Returns section in doc comment of {name}` | `:38,448-467` |
| Rust | banner (`//`, `///`, `//!`, `/*`, `/*!` forms) | `("banner", head)` | `banner comment` | `:34,487-491` |
| Rust | plain `/* */` over 2 lines | `("run", text)` | `{n}-line comment; ...` | `:492-494` |
| Rust | own-line plain `//` run over 2 lines (`///` and `//!` excluded) | `("run", texts...)` | `{n}-line comment; ...` | `:495-497` |

Exemptions: Python directives (`#!`, `-*-`, `type:`, `pragma:`, `fmt:`, `isort:`, `:20`), JS/TS
directives (`/// <`, `// @flow`, `@jsx*`, `@vitest-environment`, `@jest-environment`, `:27`), and Rust
`RS_FACING` items (clap `Parser`/`Subcommand`/`Args`/`ValueEnum`, `JsonSchema`, pyo3 and rmcp
attributes), whose doc comments callers read at run time (`:45-49,340-347,428-432`). The Rust doc rules
skip macro bodies, groups and facing scopes (`RS_UNCHECKED`, `:340-348,387-395`). Port the scope walk
with tests for enum variants, trait members, macro bodies and `macro_export`.

**Parse errors and unreadable text.** When the Python tree has an error node, the docstring rules
report nothing, as Python's `SyntaxError` path does (`comment_rules.py:166-167`). The comment rules run
on every comment node. When the hook cannot read the old text, its existing halt applies and the edit is
allowed. Python reads such a file as empty text and blocks every finding. Record that deviation.

**The rows of this plan.** Each row declares the line-comment prefixes, the block forms, the doc forms
(never counted by the run rule), the directives, and, where the group ships, the suppression, visibility
and section patterns.

| Group | Rows (U1 parity) | Rows (U2) | Declared gaps |
|---|---|---|---|
| G1 runs, blocks, banners | python, javascript, typescript, tsx, rust | bash, c, cpp, java, lua | go, ruby: `doc_attachment` |
| G2 docs | python, javascript, typescript, tsx, rust | none | bash, c, cpp, java, lua: `follow_up` |
| G3 suppressions | python, javascript, typescript, tsx, rust | none | bash, c, cpp, java, lua: `follow_up` |

Go and Ruby write doc comments as plain line comments, so the run rule would flag each doc comment over
2 lines. csharp, kotlin, php, scala and swift declare `tier_two`. hcl, json, sql, toml and yaml declare
`data_language`.

**Coverage test.** A test walks every file in `crates/syntax/languages/` and requires a row or a
declared gap for each group. A new language file fails it until it has one.

**Gaps.** A pair in a language with a gap gives a `not_checked` row with a new
`CheckGap::NoCommentRules(language, group)`. `off_by_config` gets an arm for it.

**Measurement.** `Tracked` and `Observed` in `crates/hook/src/report/log.rs` and the classifier in
`report/classify.rs` gain `comment_delta`. A finding counts as acted on when a later record for the same
path no longer carries its key.

### D4. `linter_delta` with the ruff row (U3)

**stdin for `io::run`.** `io::Spec` gains an optional stdin input. The runner writes it on its own
thread while it reads stdout and stderr, all under the same timeout, so a child that does not read stdin
cannot block the write. Tests: an input larger than the pipe capacity, and a child that never reads stdin.

**Registry row.** `opt_in`. It keeps Python paths. `RunInputs` gains the root and an injected runner, so
that tests use a fake runner and never start a process. `GatedCheck::LinterDelta` with built-in default
`advisory` lands in U3.

**Finding the linter.** The binary is `<root>/.venv/bin/ruff`, then `ruff` on the `PATH` of the hook's
environment. Never `uv run`: it costs 22 ms more per run, and it can sync or create the environment. The
config is a `ruff.toml` or `.ruff.toml` file, or a `pyproject.toml` whose parsed TOML has a `tool.ruff`
key, in the file's directory or above it, up to the root. No binary gives `no_linter_binary`, and no
config gives `no_linter_config`. Both are off by config, so the edit gets no degraded outcome (PB-85).

**The run.** From the root, with `HOME` and `PATH` from the hook's environment:
`ruff check --force-exclude --stdin-filename <path from root> --output-format json --exit-zero -`, once
for the old text and once for the new. Each run's timeout is the smaller of 2 s and half of the
remaining budget. When the budget is spent, the check starts no more runs. Parse stdout only on exit 0,
into typed findings (code, message, row, fix message). A non-zero exit, a timeout, a spent budget, or
output that does not parse gives a `linter_failed` gap, logged with the exit status and the first stderr
line.

**The diff (pure, in `crates/checks/src/linter.rs`).** Key each finding by
`(code, source line with whitespace collapsed)`. Key every `invalid-syntax` finding of one side as one
`("invalid-syntax")`. Compare old and new keys as multisets. Each new key gives one advisory with the
code, the message, and the fix message when ruff gives one.

**Severity.** ruff reports every finding as `error`, so there is no repo signal to block on.
`linter_delta` stays advisory.

**Measurement.** `Tracked`, `Observed` and the classifier gain `linter_delta`, the same way as D3.

**ruff versions.** The parser tests cover the JSON of 0.15.16, 0.16.3 and 0.17.6.

### D5. The `erosion-scan` hook (U4)

**Events.** Add `Stop` and `SubagentStop` to `Event` and `EVENT_KEYS`. Give `Hook` an `events()` list.
Make `entries()` emit one row per hook and event, and key `compare`, `state` and `run::replied` on the
hook and the event. `erosion-scan` runs on PostToolUse (matcher `Bash`), Stop and SubagentStop. An
existing install must still read as current.

**Payloads.** One parse step reads PostToolUse as session id, cwd and `tool_input.command`, and Stop as
session id, cwd and `stop_hook_active`. A missing field fails the parse, and the hook logs it and exits 0.
The erosion payload, which requires `tool_use_id`, is not reused.

**PostToolUse on Bash.** A table of shell-write forms, ported from `WRITE_SHELL`
(`comment_gate_scan.py:76-90`), decides whether the command writes. On a match the hook finds the git
roots of the working folder and of each path token, at most 6, and drops each root that
`AGENTLESS_EROSION_SCAN_IGNORE` lists (absolute paths, `:`-separated, `~` expanded). It records each root
as an exclusive-create marker, `<state>/agentless_scan/<sha256(session)>/repos/<sha256(root)>`, with the
root path as its content. It scans those roots and reports each finding once. The digest of a finding is
sha256 of the path, a NUL byte, and the key, recorded as `.../reported/<digest>`. When a marker folder is
full, the hook reports with no deduplication: a duplicate report, never a lost one. Exit 2 puts the
report in front of the agent.

**Stop and SubagentStop.** With `stop_hook_active` set, the hook exits 0. Otherwise it scans every root
marked for the session and reports every finding still on disk, with no deduplication. Exit 2 blocks
that stop attempt in Claude Code. OMP shows and logs a SubagentStop report.

**The scan.** It calls `app::lint` with `LintForm::Revision` against the closest merge base of `main`,
`develop` and `master`, local or `origin`, by the fewest commits ahead (a new function in
`crates/git/src/changes.rs`, ported from `comment_gate_scan.py:153-164`). With no base, it uses `HEAD`.
It runs only the checks in block mode for that root. It reads at most 300 files and skips a file over
400 KB, as the Python scan does. A root that sets `clone_delta` to block makes the scan build the clone
index, and that is accepted.

**Output.** The D2 layout, then "This write skipped the Write/Edit gate. Fix these now." on PostToolUse,
or "Fix these before you end the turn." on Stop.

**Deadline.** The first step of U4 measures the scan on the largest local branch, as the median and the
maximum of 5 runs. The hook deadline is at least 3 times the maximum and gives a settings timeout of at
most 15 s. If the maximum is over 3 s, U4 adds per-file stamps, as `comment_gate_scan.py:242` has,
before it goes on.

**Failures and log.** Any internal error exits 0 and is logged. Records go to
`AGENTLESS_EROSION_SCAN_LOG`. `AGENTLESS_EROSION_GATE=off` turns off `erosion-scan` as well as the
erosion hook.

### D6. Rollout and retirement (U5, owner-approved)

1. Before the release, run `agentless-mcp lint --since <base> --only comment_delta` on each local
   branch with unmerged work, and report the counts. A branch that already added long comments in bash,
   c, cpp, java or lua blocks its Stop after the first shell write.
2. Release agentless, and run `agentless-mcp hook install`.
3. In `~/.claude/settings.json`: remove the `comment_gate.py` and `comment_gate_scan.py` entries first,
   then set `AGENTLESS_EROSION_SCAN_LOG` and `AGENTLESS_EROSION_SCAN_IGNORE` (with the `omlx` root), and
   remove `COMMENT_GATE_LOG`. In `hooks_selfcheck.py`, replace `COMMENT_GATE_LOG` with
   `AGENTLESS_EROSION_SCAN_LOG` in `LOG_VARS` in the same step.
4. Move the three Python files and `comment_gate_ignore.txt` to `~/.claude/hooks/retired-<date>/`.
5. Update the hook copies in the vault's `claude-backup` set.
6. After the `linter_delta` trial clears 10% effective false positives, remove the per-file ruff step
   from each repo's `quality-gate.sh` (PB-85).

Rollback for one repo: set `"erosion_gate": {"comment_delta": "advisory"}` in its `.agentless-mcp.json`.
Rollback for every repo: set `AGENTLESS_EROSION_GATE=advisory`, which makes every check advisory. To
return to the Python gate, restore its files and settings entries from the retired folder and set
`AGENTLESS_EROSION_GATE=off`.

## Implementation Plan

### Principles to Follow

- **Functional core, imperative shell.** `comment_delta` and the `linter_delta` diff are pure. Only
  `crates/app` runs the linter, through `io::run`. (CLAUDE.md, Architecture)
- **Report only what the change adds.** Old findings never return. (Tricorder; `comment_rules.py:511-519`)
- **Guards key on the invariant.** The scan runs the checks in block mode, not a list of names.
- **Boundary integrity.** ruff's JSON and the hook payloads go through one parse step that returns
  typed values or fails.
- **Errors surface.** A linter failure is a logged gap, never a clean result.

### Patterns to Apply

- **The `contract_drift` table.** One row per language, and a declared gap for a language with no row.
- **The parity harness of the port.** Run the Python reference and the Rust check on the same corpus,
  diff the key multisets per file, and record each deviation for the owner.
- **Exclusive-create state.** The primitive in `crates/hook/src/state.rs`, not a mutable JSON file.

### Anti-patterns to Avoid

- `facts` visibility for the Rust doc rule. It counts `pub(crate)` as public.
- A run rule on Go or Ruby doc comments before attachment exists.
- `uv run` inside a hook.
- Stamps or caches before a measurement asks for them.
- A `GatedCheck` entry with no pair check behind it.

## Work Units

Start each unit in a fresh agent from this document. The owner commits.

| Unit | Content | Depends on |
|---|---|---|
| U0 | D1 per-check defaults for the 3 existing checks, D2 block text, the GatedCheck-to-PairCheck test | none |
| U1 | D3 engine, `GatedCheck::CommentDelta`, parity rows, coverage test, gate-report, parity harness | U0 |
| U2 | D3 G1 rows for bash, c, cpp, java and lua, with fixtures | U1 |
| U3 | D4 `io::Spec` stdin, `linter_delta`, `GatedCheck::LinterDelta`, gate-report (PB-86) | U1 |
| U4 | D5 measurement, multi-event hook table, `erosion-scan` | U0, U1 |
| U5 | D6 rollout and retirement | U2, U3, U4 |

U1 and U3 both edit `crates/app/src/registry.rs` and `crates/checks/src/finding.rs`, so U3 starts after U1.

## Files to Modify

Every unit:
- `CHANGELOG.md`, and the `public-api.txt` of each crate whose public surface changes.

U0:
- `crates/io/src/config.rs`: the default table (`GatedCheck` at `:102-119`).
- `crates/hook/src/modes.rs` (`:13,39-65`), `crates/hook/src/verdict.rs` (`:13,71-82,163-203`).
- `crates/prompts/data/messages.json`, `crates/prompts/fixtures/render.json`, the counts in
  `crates/prompts/src/tests.rs:106` and `xtask/src/generate.rs:227`.
- Fixtures: `crates/hook/fixtures/modes.json`, `classify.json`, `scenarios.json`, `states.json`.
- Tests: `crates/hook/src/modes/tests.rs` (`:13-72`), `crates/hook/src/verdict/tests.rs` (`:13-25,105-164`),
  and the new GatedCheck-to-PairCheck test.

U1 and U3 (each for its own check):
- `crates/checks/src/lib.rs` (`:1-2,42-55`), `crates/checks/src/finding.rs` (`CheckGap` `:193-233`),
  `crates/checks/Cargo.toml`, the new check module and its tests.
- `crates/app/src/registry.rs` (`CHECKS` `:64-78`, `PairCheck` `:82-95`, the pair table `:114-169`,
  `ALL` `:173-180`), `crates/app/src/pair_runs.rs`.
- `crates/io/src/config.rs` (`GatedCheck`), `crates/hook/src/modes.rs`, `crates/hook/src/verdict.rs`
  (`off_by_config`).
- `crates/hook/src/report/log.rs` (`:46-65,85,232,251-255`), `crates/hook/src/report/classify.rs`
  (`:50-58,135-210`), `crates/hook/src/report/tests.rs` (`:385-420`).
- `crates/prompts/data/messages.json` (`config_erosion_gate_unknown` `:218`, `lint_*_takes_pair_checks`
  `:197-200`), `crates/prompts/fixtures/render.json` (`:529-536`), the two template counts.
- `crates/cli/src/commands.rs` (`:239-297`), `crates/cli/fixtures/parser.json` (`:16-28`),
  `crates/app/data/agent-guide.md` (`:1533-1546,2013-2019`) and the generated guide.
- `crates/app/fixtures/registry.json` (`:10,17,20`), `crates/app/fixtures/capabilities.json` (`:3,6`),
  `crates/app/fixtures/guide/guide.json`, `crates/app/src/registry/tests.rs` (`:26-46`),
  `crates/app/src/lint/candidates/tests.rs` (`:388-402`).
- `tests/characterization/goldens/hooks/erosion.write.{txt,stdout}` if the mode line lists the new
  check, and their counts in `crates/app/fixtures/read/tokens.json:173-175`.
- U1 only: `crates/prompts/data/comment_rules.json`, a generator module in `xtask/src/generate/`, its
  count test, a reader in `crates/prompts/src/`, and `crates/checks/fixtures/comments.json` (recorded
  Python reference outcomes).
- U3 only: `crates/io/src/runner.rs` (stdin), `crates/prompts/data/linter_rules.json` with its
  generator module and reader, and `crates/checks/src/linter.rs`.

U4:
- `crates/app/src/hooks.rs` (`Event` `:73-89`, `Hook` `:46-62,99-158`, `entries` `:186-196`, compare and
  state `:335-412`), `crates/hook/src/run.rs` (`:151-158`), `crates/hook/src/context.rs` (`:30-73`),
  `crates/agentless/src/main.rs` (`:44-100`), `crates/hook/src/install/edit.rs` (`:152-184`),
  `crates/hook/src/install.rs` (`:104-129`).
- New: `crates/hook/src/scan.rs` (payloads, markers, output), the shell-write table beside
  `crates/hook/src/search/shell.rs`, the merge-base chooser in `crates/git/src/changes.rs`.
- Tests: `crates/app/src/hooks/tests.rs`, `crates/hook/src/install/tests.rs`,
  `crates/agentless/tests/dispatch.rs` (`HOOK_REFUSAL`), `crates/cli/tests/cli/refusals.rs`.

`xtask/` changes in U1 and U3, so `scripts/gate-selftest.sh` runs for both.

## Verification Steps

- [ ] The AGENTS.md command list in order, and `scripts/gate-selftest.sh "$gate"` for U1 and U3.
- [ ] Every golden and fixture diff predicted in writing before the run.
- [ ] U0: with no environment value and no repo entry, all three checks resolve to advisory. A block
      output ends with the fix line and has no mode line. Advisory output is unchanged.
- [ ] U1: one test per rule per parity language on old and new pairs, plus a moved comment (no finding),
      a duplicated run (one finding), a grown run (one finding), and a Python file with a syntax error.
- [ ] U1 parity: the Rust check and `comment_rules.py` agree on every local `.py`, `.js`, `.ts`, `.tsx`
      and `.rs` file, once as a new file and once on recent git history pairs. Each difference is
      fixed or listed for the owner.
- [ ] U3: PB-85's criteria (an added unused import gives one advisory naming F401 and the fix; old
      findings with no new one are clean; no linter or no config gives `not_checked` and allows the
      edit). An excluded path gives no finding. A fake runner that hangs gives `linter_failed`.
- [ ] Cost: each new check under 100 ms per edit as the median of 5 runs on an 800-line file, by
      `agentless-mcp lint --path <file> --after <new> --only <check>` (PB-84). The hook p95 before and
      after by `scripts/hook_latency.py`, against 51.6 ms (2026-11-26).
- [ ] U4: a scratch git repository, in Claude Code and in OMP. A Write that adds a 3-line comment is
      blocked. A `cat >` that adds one gets a PostToolUse report. A Stop attempt is blocked once, and the
      next attempt with `stop_hook_active` passes. In OMP a SubagentStop report is shown and logged.
- [ ] The release regression gate. The retrieval tier must show zero differing instances, because this
      work changes no ranking.

## Risks

- `linter_delta` runs each repo's own ruff binary on every edit (decision 9). The venv comes from the
  owner's own `uv sync`, so the binary comes from the repo's lock file.
- Blocking on style can stall an agent on a false positive. gate-report measures each block from U1.
- The multi-event hook table changes the install and status rows of every hook.
- If OMP ever sends `stop_hook_active` as false on a continuation, a Stop block on a finding the agent
  cannot fix loops. The Python scan has the same dependency today.
- Silent allows under a default block: old text over the size cap or not UTF-8, a payload over 4 MiB, and
  the lint deadline allow the edit and show the agent nothing (`crates/hook/src/destination.rs:100-105`).

## Ticket Edits

Done 2026-11-27. The original PB-84 description is saved in the session scratchpad.

- PB-103 under PB-84: move the comment gate into the erosion gate. Sub-tasks PB-104 (U0), PB-105 (U1),
  PB-106 (U2), PB-107 (U4), PB-108 (U5). U3 is PB-86.
- PB-109 under PB-84: the remaining languages (below), with sub-tasks PB-110 to PB-113.
- PB-84: the Scope gains the comment work and states that `comment_delta` blocks by default, as an
  exception to "each check stays advisory until its trial".
- PB-85: JS/TS moves to the follow-up, and the OMP claim about `quality-gate.sh` gets a correction.
- PB-86: oxlint moves to the follow-up.

## Follow-up (PB-84)

- G2 and G3 rows for bash, c, cpp, java and lua.
- G1, G2 and G3 rows for Go and Ruby, with a doc-attachment rule. `crates/facts/src/extract/attached.rs`
  attaches overloads and decorators today, not comments.
- A pair path for the tier-2 languages (C#, Kotlin, PHP, Scala, Swift), then their rows. With it, add
  `multiline_comment` to `kotlin.toml` and `swift.toml`: agentless does not see Kotlin or Swift block
  comments as comments today.
- The JS/TS linter row: oxlint by a temporary file, or eslint by stdin, measured on a real repo.
- A per-file linter row with a measured cost, or a declared gap, for each other tier-1 language.
- `.pyi`, `.jsx`, `.mts` and `.cts` extensions, with their own retrieval-tier proof.

## Validation Record (2026-11-27)

Fable orchestration with Sonnet workers and a 3-validator panel. Verdict CONCERNS. The local peer step
did not run: a safety classifier stopped its one call. Each finding and its result:

- High, `io::run` has null stdin: D4 adds stdin to `io::Spec` (U3).
- High, 5 of the 10 U3 languages are tier 2: U2 ships bash, c, cpp, java and lua. Tier 2 and the
  Kotlin and Swift kind fix moved to the follow-up.
- `--force-exclude`: added to the ruff command, with a test.
- Registration and surfaces: both checks are `opt_in`, `RunInputs` gains the root and an injected runner,
  and the timeout follows the remaining budget.
- gate-report cannot see new checks: D3 and D4 add them to `Tracked` and `Observed`. Scan blocks are out
  of gate-report, stated as a trade-off.
- Extensions: removed (decision 10).
- Unit order: U0 changes no behavior, each `GatedCheck` lands with its pair check, a test links the two
  tables, and U3 follows U1.
- Block text: D2 gives the layout and the fix text.
- Parity specification: D3 gives the rule table, the branch order, `keeps`, the parse-error rule, the
  unreadable-text deviation, and no `facts` dependency.
- File list: completed, with no reference to an agent map that a fresh agent cannot read.
- D5 details: the payloads, caps, digest, full-folder behavior, ignore setting and a measured deadline.
- D4 gaps: no binary and no config are off by config, config detection parses TOML, `invalid-syntax`
  is one key, parse only on exit 0.
- Ticket departures: listed under Ticket Edits.
- Scan rollout: D6 step 1 counts the findings per branch first.
- Latency bars: written in Verification Steps.
- Stop wording: one block per stop attempt, with Claude Code and OMP split.
- D7 order and the env warning: D6 step 3 and D1.
