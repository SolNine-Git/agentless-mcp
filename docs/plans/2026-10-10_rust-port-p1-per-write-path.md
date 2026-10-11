# Rust Port, Phase 1: The Per-Write Path

**Date**: 2026-10-10 | **Status**: not started | **Overview**: [2026-10-10_rust-port-00-overview.md](2026-10-10_rust-port-00-overview.md) | **Depends on**: P0 (0.10.1 released, workspace and gates live) | **Branch**: `feat/rust-port` | **Est. Rust lines**: about 8,000

## Goal

Replace the per-write path with one Rust binary. The binary runs the three pair checks (`complexity_delta`, `contract_drift`, and `clone_delta`) and the `lint` and `capabilities` commands. It also runs the erosion hook in process, reading the Python `tags.db` as decision 9 says. P1 passes only when a measured hook call meets the latency gate in each cache state, and when the tokenizer and the pair checks match the 0.10.1 parity base.

## Scope

- **In**: the crates `pycompat`, `prompts`, `syntax`, `metrics`, `io`, `git`, `store` (the read-only `tags.db` reader), `checks`, `app` (lint pair forms, capabilities), `render` (lint, capabilities, envelope), `cli` (lint, capabilities), `hook` (erosion gate), and `agentless`.
- **Out**:
  - The Rust store's own file, its writer, and `index` (P2). Until P4, the Python MCP server and Python `agentless-mcp index` are the only writers of the cache the hook reads.
  - Symbol facts, the graph, and every read operation (P2).
  - The `--candidates` lint form, which needs SEARCH/REPLACE parsing (P3).
  - The seven patch checks (P3).
  - The MCP server and the search-gate hooks (P4).

## Crates at the end of P1

| Crate | Layer | Depends on (P1 subset) |
|---|---|---|
| `kernel` | L0 | none (from P0) |
| `pycompat` | L0 | none |
| `prompts` | L1 | kernel |
| `syntax` | L1 | pycompat, kernel |
| `io` | L1 | kernel |
| `metrics` | L2 | syntax, pycompat, kernel |
| `git` | L2 | io, kernel |
| `store` | L3 | metrics, io, kernel |
| `checks` | L3 | metrics, syntax, prompts, kernel |
| `app` | L5 | checks, store, git, io, metrics, prompts, kernel |
| `render` | L6 | app, prompts, pycompat, kernel |
| `cli` | L7 | app, render, prompts |
| `hook` | L7 | app, render, prompts, io |
| `agentless` | L8 | cli, hook, app, store, git, io |

Each dependency exists when its user is built. `cargo xtask layers` checks this on every commit.

## Binary name

The `agentless` crate builds a binary named `agentless-mcp`, so commands and hook settings keep their spelling. Until P4 the Python 0.10.1 tool owns `agentless-mcp` on `PATH`. The Rust binary is therefore installed outside `PATH`, and called by absolute path from the hook settings and the latency script.

## Carried from P0d

- When `syntax` adds the tree-sitter crate, set the workspace `rust-version` to that crate's minimum.
- `crates/io/clippy.toml` (the root file without the `std::fs` entries) and `crates/store/clippy.toml` (without `rusqlite::Connection::open`) arrive with their crates.
- `cargo xtask goldens --update` and self-test violation 14 (a changed golden file) arrive with the first golden files.
- Generated Rust is committed source. The gates refuse `include!`, so no crate reads code from `OUT_DIR`. `cargo xtask generate` writes the Rust source into `crates/<crate>/src/generated/`, and the commit holds the file. An xtask test fails when a committed file differs from a fresh generation. The static gates and clippy then read the generated code as they read any other source. Only `syntax` keeps a `build.rs`, which compiles the grammar C sources with `cc` and writes no Rust (work items 2 and 3).
- Gaps that the P0d gates leave open:
  - No gate checks the insta config. `.config/nextest.toml`, where a default filter can skip tests, is a policy item, so it changes only with a `Policy-Change` trailer.
  - No gate checks `[package.metadata.cargo-machete] ignored`, which hides an unused dependency.
  - No gate checks a per-package `rust-version` (clippy's MSRV) or `edition`, which change what the lints check.
  - No gate checks the environment or config above the repository root (`CLIPPY_CONF_DIR`, `RUSTFLAGS`, `CARGO_HOME` config). CI's `rust` job sets none of them, and nothing checks that.
  - The token scan reads source text, so the gates do not detect a proc-macro crate that builds an attribute from strings, or a third-party macro that expands to an attribute or to `unsafe` code. rustc still rejects a suppression of a lint at `forbid`, a macro-built one too (E0453). The workspace has no proc-macro crate.
  - No lint refuses `Command::output()` outside `xtask/src/process.rs`, the one subprocess helper.
  - rustdoc compiles doc examples, but the token walker reads their text as a literal, and nextest does not run doctests.
  - The build-script rule lists only `syntax`, and that crate's `build.rs` can still set a cfg that hides code from clippy.
  - clippy runs the dev profile with the default features, so it does not lint code under `not(debug_assertions)` or behind a feature that is off (derived, not run). Run clippy with `--release` or `--all-features` when those builds exist.
  - `cfg_select!` is unstable on 1.94.0 (E0658), and the cfg rule does not read it. Add it to the cfg rule when the toolchain moves to a release that accepts it.
  - A frontmatter block (`---` at the top of a file) passes the gates. rustc 1.94.0 rejects it (E0658). Check it again when the toolchain moves.
  - An ignored, untracked config file other than a clippy config passes the local gates, because the tool-config check lists files through git's ignore rules. CI is unaffected, because its checkout holds no ignored file.
  - A policy file that the base lacks is accepted as its creation commit wrote it.
  - When the base has no `crates/erosion-baseline.json`, the commit that creates it sets the first ratchet reference. The gate bounds the baseline by `health` only while no committed version exists (derived from `xtask/src/erosion.rs`, not run).
  - On a pull request, CI's HEAD is GitHub's merge commit, which carries no trailer. If main and the pull request both change one policy file, and git merges them cleanly, the policy gate fails on that merge (derived from `xtask/src/policy.rs`, not run).
  - The gate code is in the tree. A change that rewrites xtask, `ci.yml`, or the self-test can loosen the gates that judge it. No CODEOWNERS file or branch protection asks for a review of those paths.
  - The quality jobs restore the grammar cache through a `restore-keys` prefix, so they can restore a cache that another key saved. Nothing verifies the grammar bundle that `agentless-mcp warmup` downloads.
  - xtask builds on Unix only, because it puts each child process in its own process group through `std::os::unix::process::CommandExt`. CI runs Linux and macOS (overview decision 5).
  - The self-test stops each command at 120 s. The figure is derived: about ten times the slowest cold command measured with one build job (11.4 s). If a command in a cold CI run takes longer, raise `step_deadline` from the row times in that run's log.
  - CI builds cargo-public-api 0.52.0 from source with `cargo install --locked`, because install-action does not ship it. rust-cache keeps the binary, so only a cache miss builds it again.
  - CI's `cargo metadata --locked` step proves `Cargo.lock` before the first build, but the `cargo +nightly public-api` run inside xtask takes no `--locked` flag.
  - Self-test violations 7, 9, and 12 still match short texts (`public-api.txt`, `disallowed`, `itoa`) that another failure can also print.

## Work items, in build order

### 1. `pycompat`

- **Functions**: `float_repr`, `round_half_even`, `sum_builtin` (CPython 3.12+ built-in `sum()` of floats, which is compensated), `splitlines`, `split_whitespace`, `strip`, `PyJson` (a serde_json `Formatter`: `indent=2`, `ensure_ascii` with surrogate pairs, `": "` separators), `max_first`, and `python_sloc`. A plain `+=` loop in Python is a plain Rust loop, not `sum_builtin`. Each call site in the port names which of the two it reproduces.
- **`python_sloc(src) -> Result<BTreeSet<u32>, TokenizeError>`**: the CPython 3.13 tokenize rule, on lines from `splitlines(keepends=True)`.
  - A line counts when a token other than COMMENT, NL, NEWLINE, INDENT, DEDENT, or ENDMARKER starts on it.
  - Covers string prefixes, triple quotes, f-strings with nesting (PEP 701), line continuation, brackets, and indentation errors.
  - Rejection matches `tokenize.TokenError` and `SyntaxError`, including `IndentationError`, as a `TokenizeError` kind.
- **Python sources**: `src/agentless_mcp/core/complexity.py:298-323` (the SLOC rule); `src/agentless_mcp/application/envelope.py:457-459` and `render.py:77-86` (JSON form and negative zero).
- **Fixture generator** (development time only): `scripts/pycompat_fixtures.py`.
  - It runs under a pinned CPython 3.13 through `uvx`. CI also tests Python 3.10, where `sum()` is not compensated; the fixtures and goldens come from 3.13 only.
  - It writes `crates/pycompat/fixtures/*.json`: inputs and expected outputs for every function.
  - The tokenizer corpus is every `.py` file in the fixtures, a frozen copy of mcp-local `src/` at f17af96, and the Python files of the Loc-Bench checkouts.
  - Tests read only the committed JSON. No test runs Python.
- **Fuzz job** (opt-in, nightly): generate Python source with proptest. Compare `python_sloc` with `python3 -m tokenize`, and store each mismatch as a new fixture.
- **Done when**: every fixture passes, and the corpus shows zero differences in line sets and rejection kinds.

### 2. `prompts`

- `cargo xtask generate` reads `src/agentless_mcp/prompts/*.json` (the files move into the crate when the Python tree is deleted). It writes `crates/prompts/src/generated/` with one typed function per message, and each placeholder becomes a typed argument. The commit holds the generated file ("Carried from P0d").
- A missing or extra placeholder fails the generator. `deny_unknown_fields` applies, because the project owns these files.
- `contract_rules()` returns the parsed `contract_rules.json`, validated eagerly.
- **Tests**: render every message once; parse every contract rule.

### 3. `syntax`

- **`Language`**: an enum of the 22 languages, with `from_path` and `from_name`.
- **`LangSpec`**: one TOML file per language in `crates/syntax/languages/`. `cargo xtask generate` turns them into static tables in `crates/syntax/src/generated/`, under the same test as `prompts`. The P1 fields are those that metrics and checks read:
  - extensions and tier;
  - comment, identifier, literal, unit, bound, scope, and descend kinds;
  - decision points, with `Cond` predicates and field and token operators;
  - clone candidate kinds and clone literal classes;
  - the docstring rule and the SLOC rule.
- **Sources for the tables**:
  - `src/agentless_mcp/core/complexity.py:501-775` (RULES)
  - `src/agentless_mcp/core/clones.py:204-298` (CANDIDATES)
  - `src/agentless_mcp/core/normalize.py:265-393` (clone identifiers and literals)
  - `src/agentless_mcp/core/extractor.py:109-125` (comment and indent-block kinds)
  - `src/agentless_mcp/core/grammars.py:78-115` (tiers)
- **Code, not data**:
  - `Cond`, the six predicates at `complexity.py:472-498`;
  - `NameRule`, the name and binding rules at `complexity.py:381-470`. The Rust impl owner uses the one rule that P0b B3 settled.
- **Grammars**: the C sources vendored from `parser-sources-1.14.3.tar.zst`, with `ctype_shim.h` and utf8proc, compiled by `cc` in `build.rs` and checked against a sha256 manifest (overview, Grammar sources). The build script compiles C only and writes no Rust. `tree-sitter` is pinned to `=0.26.9`. `grammar(Language)` performs no IO.
- **`parse(lang, src) -> Result<Parsed, Unparsable>`**. `Parsed` owns:
  - the `Tree`;
  - `Flat`, built in one cursor walk: kind ids, a named bit set, spans, the subtree end index per node, and a field index for name lookups;
  - the source as `Arc<str>`.
- **Queries**: compiled once per language and kind, and validated at start-up.
- **Unsafe**: grammar FFI is the only `unsafe` in the workspace. It lives in `crates/syntax/src/ffi.rs`, under the lint table that P0 D2 sets for `syntax`.
- **Tests**:
  - Every kind name in every `LangSpec` exists in its grammar (`Language::id_for_node_kind`).
  - Every query compiles.
  - On every fixture file, the `Flat` subtree slices equal a cursor walk (proptest over random node indices).
  - Record the `Flat` memory per node on the largest fixture.

### 4. `metrics`

- **`measure(&Parsed) -> Result<FileMetrics, Unmeasurable>`**: one traversal of `Flat` gives:
  - units: qualname, span, body key, cc, and sloc;
  - blocks: span, exact key, Type-2 key, token count, and whether the block is a function;
  - the SLOC line set.
- Nested keys hash their own slices. Python SLOC uses `pycompat::python_sloc`, then removes docstring lines with the ownership test. The byte-column slicing quirk stays on purpose (`complexity.py:356-379`).
- Also `erosion(units, threshold)`, `clone_groups(files)`, and `clone_sloc_lines(...)`.
- **Python sources**:
  - `complexity.py` (`_measured` 132-163, `_walk_units` 185-206, `_decisions` 256-266, `_weight` 269-281, `_leaf_sloc_lines` 284-295, docstrings 330-379)
  - `normalize.py` (`node_stream` 258-262, `_tokens` 555-591, `_directive_text` 594-607, `clone_key` 405-439)
  - `clones.py` (`file_blocks`, `_blocks` 150-165, `clone_groups`, `clone_sloc_lines`)
- **Parity tests**:
  - The 6 complexity goldens (`tests/characterization/goldens/complexity/`): units both ways, cc and sloc exact, erosion within 1e-9, clone groups and lines exact, and `tokenize_rejects` as a typed error.
  - The scb-check snapshot (f17af96 mcp-local `src/`): 1,770 units, 200 groups, 545 instances, and 2,513 clone lines.
- **Property tests**:
  - A comment-only edit keeps the body key.
  - Renaming identifiers keeps the Type-2 key.
  - Reordering units does not change the sorted set of (qualname, cc).

### 5. `io`

- **`Runner`**: one bounded process runner (process-wrap, wait-timeout). `Spec` carries the program, args, cwd, environment allow-list, timeout, stdout cap, and process-group flag. `Outcome` is `Exited`, `Truncated`, `TimedOut`, or `NotRun`.
- **`Fs`**: `walk` (bounded, with the (dev, ino) set), `read` (`ReadOutcome`: text, too large, not UTF-8, or unreadable; the cap is 1,000,000 bytes, as `src/agentless_mcp/util/fslimits.py:35`), `contained` (refuses a path that resolves outside the root), and `append_line` (the hook log). `io` is the only crate that writes files (P0 D2).
- **Config**:
  - `EnvConfig::from_env(get)`: read once at start-up; a bad value fails start-up.
  - `parse_project(bytes)`: serde. An unknown key is dropped with a warning, as `KNOWN_KEYS` does today. The open uses the P0a A2 rules (regular file, contained, `O_NONBLOCK`).
- **Lock and deadline**: `try_lock` (fs4); `Deadline` (Instant, credit, cancel flag), passed explicitly; `walk_budget(remaining)`, the lesser of 10 s and half the remaining allowance (`src/agentless_mcp/util/deadline.py:26`, 93-97).
- **Python sources**:
  - `src/agentless_mcp/core/gitinfo.py:356-470` and `src/agentless_mcp/core/sandbox.py:374-633` (the two runners this replaces);
  - `src/agentless_mcp/util/fslimits.py`, `src/agentless_mcp/core/treewalk.py`, `src/agentless_mcp/core/projectconfig.py`;
  - `src/agentless_mcp/util/cachedir.py:32-55`, `src/agentless_mcp/util/deadline.py`.
- **Tests**:
  - One contract suite runs against the real `Fs` and `Runner` and against their fakes.
  - Timeout and kill behavior use a child that sleeps, with no assertion on wall time beyond the timeout.

### 6. `git`

- **`GitRead`**:
  - `probe(root) -> Result<RepoState, ProbeError>`, with `RepoState::NoGit` as an answer. Python uses four processes and keeps short object ids (`gitinfo.py:284-318`). The Rust probe uses two: `rev-parse --show-toplevel HEAD HEAD^{tree}` and `status --porcelain`. It keeps full ids and shortens them only for display. This is new behavior; the receipt line must stay byte-equal, and the parity harness checks it.
  - `list` (`ls-files --cached --others --exclude-standard -z`).
  - `blob` (`cat-file blob`, bounded).
  - `changes(rev)` (`diff --raw -z -M --relative rev --` plus untracked files).
  - `verify(rev)` (`rev-parse --verify --end-of-options rev^{commit}`).
- **Hardening, kept exactly**: the argv prefix from `src/agentless_mcp/core/gitinfo.py:76-90`; `LC_ALL=C`; every `GIT_*` variable stripped except `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_NOSYSTEM`; the `./` pathspec prefix.
- **Pure parsers**: status count, NUL listing, raw diff records (modes 160000 and 120000), and the binary test (a NUL byte in the first 8,000 bytes).
- **Python sources**: `gitinfo.py:284-318` (snapshot), `src/agentless_mcp/core/gitchanges.py`, and `src/agentless_mcp/core/githistory.py:75-94`.
- **Tests**: parsers on captured output; one integration test per query on a repository the test builds in a temporary directory.

### 7. `store`: the `tags.db` reader (decision 9)

- **`TagsDb::open(root) -> Option<TagsDb>`**: opens the Python cache at the path `src/agentless_mcp/util/cachedir.py` computes, read-only (`mode=ro`, `query_only`). It returns `None` for a missing file, a `meta` schema other than 17 (`src/agentless_mcp/core/cache.py:185`), or a read error, and the caller reports the `no_tag_cache` gap. SQLite WAL readers do not wait for the writer.
- **Reads**: the `files` rows (path, sha256) and the `blocks` rows of each file (`cache.py:2117-2220`). `blocks_for(path, text)` returns the stored blocks when the row's sha256 equals the text's. Otherwise the caller measures the text in memory and writes nothing, as `cache.py:597-606` does.
- **No writer in P1.** The P2 store adds the Rust file, the writer, plan, and eviction.
- **Tests**:
  - A fixture `tags.db`, written by 0.10.1 `index` over a fixture repository and committed: its blocks equal a re-measure of the same files.
  - Schema 15 and 17, a missing file, and a corrupt file each give `None`.
  - A row whose sha256 differs from the text is measured in memory.

### 8. `checks`

- **Types**: `Check` trait, `Needs`, `Inputs` (built lazily for the selected checks), `Finding` and `Detail` (serde), and `Finding::not_checked`. `run(selected, inputs)` applies one error guard and one sort.
- **`cascade<T: Keyed>(old, new, stages)`**: the one matcher. A `Stage` has a scope (per file or global), a key, a mode (first-in-first-out, unique-then-rank, or grouped), and a label. A grouped stage pairs inside each group of its key by a second key first, in order, then pairs the rest in order: the body pass pairs same-qualname units first inside each body-key group (`src/agentless_mcp/core/erosion.py:509-537`). It replaces `erosion.py:484-581` and `837-880`, with `VecDeque` in place of `list.pop(0)`.
- **Path scope**: one function per check, which implements decision 8 as P0a A1 settled it.
- **`complexity_delta`**: the entry at `erosion.py:348-365`, and the body, name, and collision passes at 484-562 (the D2 spec in `docs/plans/2026-10-09_erosion-checks.md`).
- **`contract_drift`**: rules from `prompts::contract_rules()`, run as queries on the `Tree`; hits keyed by (rule, unit, capture), compared as multisets across one run's files; `not_checked` details `no_boundary_globs` and `no_rules_for_language`.
  - **Glob rules** (`src/agentless_mcp/core/contracts.py:40-45`, 156-198), not a glob crate: a glob and a path split on `/`; `*` matches inside one segment; `?` and `[` are literal; a final `**` matches one or more non-empty segments, and an inner `**` matches zero or more; the match is one pass with no backtracking. Port the `TestBoundaryGlobs` cases (`tests/unit/test_contracts.py:631-680`) as a table.
- **`clone_delta`**:
  - Line pairing uses `similar` with `Algorithm::Histogram`, pinned to one exact version (decision 6).
  - Then exact, Type-2, and cross-file stages.
  - Nesting fold; the 40-token floor; at most 3 evidence sites and 20 detail sites.
  - **The index, as at `1dc4d70`**: built lazily, on the first `clone_delta` call with a changed file in scope (`src/agentless_mcp/application/lint_service.py:167`, 345-349). It indexes files in sorted order until `walk_budget` runs out, and each file after that gets the `clone_index_stopped` gap (`erosion.clone_index`, `erosion.py:377-400`). The hook form reads blocks through `TagsDb` only, and without it reports one `no_tag_cache` row per pair (`lint_service.py:351-355`; `erosion.py:410-411`, 954-958). `--since` uses `TagsDb` when it exists and measures in memory otherwise.
- **Message text**: from `prompts`. The finding sort orders match `erosion.py`.
- **Tests**:
  - A table of (old text, new text) to findings per check, for all 12 tier-1 languages.
  - Property tests: a moved unit, a renamed unit, and reordered same-name units give no finding.
  - One test pins the move-plus-copy case from the decision 6 research.
  - The clone index with no cache, a fresh cache, a stale row, and a budget that runs out after one file (injected clock).

### 9. `app` (P1 operations)

- **Types**: `Services` (the port bundle) and `Session` (`RepoState`, `FactSource`, `Deadline`).
- **`op!` macro**: one declaration per operation. Each parameter carries a type, default, bounds (`Limit`), description key, CLI spelling, MCP spelling, and omission rule. The macro emits the serde request struct, the clap arguments, the JSON schema (schemars), and an `OpSpec` entry in `OPS`.
- **Operations**:
  - `lint` in two pair forms: `--path REL --after FILE` (the hook form) and `--since REV`. Flags `--only` and `--fail-on`.
  - `capabilities`: version, `lint_checks` with each input kind, effective config with warnings, and the language capability report.
- **Python sources**:
  - `src/agentless_mcp/application/lint_service.py` (registry 138-188, selection and the two index sources 329-356, pair forms 707-921, `_run_pairs` 959);
  - `src/agentless_mcp/application/capability_service.py`.
- **Tests**:
  - Every operation runs against fake ports (fake `Fs`, `GitRead`, `TagsDb`, and `FactSource`), with insta snapshots of the typed result.
  - A generated cross-door test sends every parameter through the CLI spelling at 0, -1, the cap + 1, and absent.

### 10. `render` (P1 views)

- **Envelope and receipt**: `wrap` and `wrap_json`, with the head, dirty, and cache line formats, the 16,000-token ceiling, the 1/8 block share, the 64-token marker, and at most 8 config warnings (`src/agentless_mcp/application/envelope.py:72-105`, 342-421).
- **Lint text and JSON**: `LintFinding`, `LintCandidate`, and `LintReportView` (`src/agentless_mcp/application/render.py:1434-1580`) and `render_lint` (1581), with `detail` and `observed`.
- **Capabilities** text and JSON.
- **Tests**: insta snapshots over fixed values; the receipt line formats.

### 11. `cli` (P1 commands)

- **`command()`**: built from `OPS`.
- **Exit codes**: 0 ok, 1 operation failed, 2 usage, 3 `--fail-on` matched. Notes go to stderr, with the `agentless-mcp: ` prefix.
- **Python sources**: `src/agentless_mcp/adapters/cli/main.py` (lint 604-659, 1341-1392), and `src/agentless_mcp/adapters/cli/formatting.py:13-23`, 77-97.
- **Tests**: trycmd snapshots of `--help`, usage errors, and exit codes.

### 12. `hook` (erosion gate)

- **Command**: `agentless-mcp hook erosion`. It reads the PreToolUse payload from stdin, capped at 4 MiB. Required fields are validated, and extra fields are ignored (Codex V35).
- **`proposed_text`**: the Write rule (content verbatim) and the Edit rule. The Edit rule:
  1. counts CRLF and bare LF in the old text;
  2. normalizes to LF;
  3. replaces `old_string` once, or every occurrence with `replace_all`;
  4. converts to CRLF when CRLF was the majority (a tie gives LF);
  5. skips the Edits that Claude Code itself fails.
- **Environment**: `AGENTLESS_EROSION_GATE` and `AGENTLESS_EROSION_LOG`, as `contrib/hooks/agentless_erosion_gate.py:26-28` reads them. `AGENTLESS_EROSION_BIN` has no use in process; the Rust hook ignores it, and the agent guide says so.
- **Steps**:
  - Mode resolution: the repository `erosion_gate` entry, then `AGENTLESS_EROSION_GATE`, then advisory. `off` exits before any work.
  - Run the pair checks in process, with the clone index from `TagsDb` only.
  - Classify the outcome as blocked, advisory, degraded, clean, or skipped. A gap in `OFF_REASONS` (`agentless_erosion_gate.py:60-63`: `no_boundary_globs` and `no_rules_for_language` for `contract_drift`, `no_tag_cache` for `clone_delta`) means the check is off for that file, so it does not make the outcome degraded.
  - Output: advisory `additionalContext` JSON on stdout and exit 0; block with exit 2 and findings on stderr. At most 15 findings.
  - The JSONL record keeps the schema `contrib/hooks/erosion_gate_report.py` reads. `binary` is the current executable. `dropped` lists each requested check that lint does not offer as a pair check (`agentless_erosion_gate.py:663-667`); in process every gated check is offered, so it is empty, and a test pins that.
- **Limits**: the 10 s overall deadline (`DEADLINE_S`) with the 0.5 s log reserve (`LOG_RESERVE_S`) and the 2 s git timeout (`GIT_TIMEOUT_S`, `agentless_erosion_gate.py:36-40`). The Python hook calls lint as a subprocess with no active `Deadline`, so its clone index gets the full 10 s budget. In process the hook passes no call allowance to `walk_budget`, so the budget stays 10 s and the outcomes match. SIGTERM caught (signal-hook): log `degraded`, exit 0; fail open on any internal error.
- **Python source**: `contrib/hooks/agentless_erosion_gate.py` (whole file); spec D8 in `docs/plans/2026-10-09_erosion-checks.md:305-336`.
- **Tests**:
  - Edit emulation against a naive model (proptest), including CRLF majority and the tie.
  - Payload fixtures: valid, malformed, extra fields, and over the cap.
  - Each outcome in all 12 tier-1 languages, and the `no_tag_cache` row giving a clean outcome.
  - The deadline with an injected clock.
  - SIGTERM sent to a child process.
  - The log record schema checked by running the Python report over Rust-made records (development time).

### 13. `agentless` and the switch

- `main` reads argv and `EnvConfig` once, builds `Services` from the real ports, and dispatches to `cli` or `hook`.
- `contrib/hooks/settings-example.json`: the erosion hook entry calls the Rust binary by absolute path, `hook erosion`, with a timeout of 15 s.
- The Python hook script stays for users of the Python package until P4.

## Parity harness

- **Pair-check rows**: the lint goldens are SEARCH/REPLACE candidates, which P1 cannot parse. So `scripts/pair_parity_fixtures.py` runs once under the frozen 0.10.1 build. It writes each golden candidate's (path, old text, new text) pairs and the three pair checks' rows and `observed` lists to `crates/checks/fixtures/`. A Rust test compares the rows after `PyJson`: checks, paths, lines, messages, counts, and `observed` byte for byte. For `clone_delta`, the reported copy site may differ only on the fixtures listed in `crates/checks/fixtures/accepted.toml`, each with the reason (decision 6). Every other field is exact.
- **`--since`**: a script builds a git repository with six commits over the fixtures and runs both builds with `--since HEAD~5 --json`. Before the comparison, it asserts that both builds exit 0 and that each of the three checks reports at least one row. The output must be equal, apart from the receipt line.
- **Hook**: a payload corpus of about 40 Write and Edit calls across the 12 languages runs through both hooks, in three cache states over the same `tags.db`: fresh (0.10.1 `index`), stale (0.10.1 `index`, then one file in ten changed), and absent (an empty `XDG_CACHE_HOME`). Decisions, stdout, stderr, and log records must be equal, apart from `ts`, `latency_ms`, and `binary`.

## Exit gates

- [ ] **Latency**. `scripts/hook_latency.py` uses the overview's baseline method. It runs the Python 0.10.1 hook and the Rust hook interleaved, 30 runs each, on three repositories: a frozen copy of this repository, the mcp-local clone at f17af96, and the dgl Loc-Bench checkout. Each repository runs in the three cache states, which the script seeds with 0.10.1 `index`.
  - On mcp-local, in each cache state: p50 50 ms or less and p95 100 ms or less (proposed; the owner confirms before P1 starts).
  - The other repositories are recorded, with no threshold.
  - Medians, p95, and the commands go in CHANGELOG.md.
- [ ] **Hook outcome suite** passes in all 12 tier-1 languages: Edit emulation, advisory and block output, the `no_tag_cache` row, malformed and extra-field payloads, deadline and SIGTERM, the log record schema, and degraded analysis.
- [ ] **Tokenizer corpus** shows zero differences against pinned CPython 3.13, and the fuzz job runs.
- [ ] **Complexity goldens** (6) and the scb-check snapshot match.
- [ ] **Pair-check parity rows** (with the accepted list), `--since` parity, and hook parity in the three cache states are equal.
- [ ] **Recorded**: binary size and `Flat` memory per node.
- [ ] **Gates**: `cargo xtask layers`, `public-api`, `suppressions`, `lint-tables`, and clippy pass. The erosion gate (pinned 0.10.1) passes on `crates/`. The mutation job (overview rule 11) has run once and set its floor.
- [ ] **Release**: the merge commit is tagged `rust-p1` (overview rule 1). From then on, CI builds the lint gate from that commit (overview rule 2).

## Risks

| Risk | Response |
|---|---|
| The tokenizer differs from CPython on a rare form | The corpus and the fuzz job find it; each mismatch becomes a fixture. The gate needs zero differences. |
| The latency gate fails | Profile before changing design. The likely costs are the repository listing and reading every in-scope file for the clone index (`lint_service.py:359-373`). |
| Histogram pairing differs from SequenceMatcher on a real edit | Counts never change. A different reported copy is accepted (decision 6) only for a fixture on the accepted list, and the move-plus-copy test pins the chosen case. Python's `SequenceMatcher` keeps `autojunk` on: in a new text of 200 or more lines, a line that makes up more than 1% of it leaves the match index, and Histogram has no such rule. A fixture of 200 or more lines with many brace-only lines pins that case. |
| A Python change bumps the `tags.db` schema during P1 to P3 | The reader accepts schema 17 only and reports `no_tag_cache` otherwise, so the hook stays safe. Overview rule 7 forbids the bump without a matching reader change. |
| `Flat` memory is too high on large files | It is about 22 bytes per node (estimate); P1 records it. If needed, drop `Flat` for files over a size and walk the `Tree` instead. |
| The vendored grammar sources do not build on macOS | The D1 build was measured on Linux only. Build them on the macOS CI row before P1 relies on them. The scanners of ruby, scala and swift export globals without a language prefix; none collide among the 22. |
