# Rust Port, Phase 3: The Write Surface

**Date**: 2026-10-10 | **Status**: done on `feat/rust-port`, merged in PR #62 (`021548f`) and released as 0.11.0rc1 | **Overview**: [2026-10-10_rust-port-00-overview.md](2026-10-10_rust-port-00-overview.md) | **Depends on**: P2 released | **Branch**: `feat/rust-port` | **Est. Rust lines**: about 3,500

## Goal

Port the operations that read or apply candidate edits:
- the `--candidates` lint form, with all ten checks;
- `patch` (parse, check, apply, normalize);
- `validate`, with worktree sandboxes and test runs;
- `vote`.

Every write into the target repository goes through one transaction that rolls back on failure. P3 passes when the lint golden is byte-identical and the patch parity fixtures match. It also needs write failure, rollback, and process-tree cancellation to pass through the ports.

## Scope

- **In**: the crates `patch` and `patchcheck` are new.
  - `io` gains `write_atomic` and `Txn`.
  - `git` gains worktrees.
  - `app`, `render`, and `cli` gain the write operations.
- **Out**: the MCP server and the search-gate hooks (P4).

## Crates at the end of P3

| Crate | Layer | New dependencies in P3 |
|---|---|---|
| `patch` | L2 | syntax, kernel |
| `patchcheck` | L4 | checks, facts, graph, patch, metrics, syntax, kernel |
| `app` | L5 | patch, patchcheck |

## Work items, in build order

### 1. `patch`

- **`parse_blocks(text) -> BlockParse`**: SEARCH/REPLACE blocks (`src/agentless_mcp/core/patches.py:254-563`; `parse_blocks` has cc 11 and is split).
- **`parse_unified_diff(text) -> DiffParse`** and `orientation` (`src/agentless_mcp/core/unidiff.py:147-560`, with `_hunk_body` at 871). `_headers` (cc 13) and `_hunk_body` (cc 14) are split. The CRLF refusal (unidiff.py:309) stays.
- **`find_whole_lines(text, block) -> Hits`**: one function for the whole-line match rule. It replaces three homes (finding C10): `patches.py:647-731`, `src/agentless_mcp/core/patchlint.py:869-887`, and `unidiff.py:245`. The "empty SEARCH anchors nowhere" refusal is stated once.
- **`resolve_elisions`** (`patches.py:734-800`): the scope loop goes, because every caller passes the whole file (finding C10).
- **`apply(edits, files) -> ApplyResult`**: pure. Writing belongs to the caller through `io::Txn`.
- **Keys**:
  - `file_key` and `equivalence_key` (`src/agentless_mcp/core/normalize.py:200-232`) hash a unified diff of the normalized token streams.
  - The diff comes from `similar`, formatted to Python's `unified_diff(lineterm="")` and split with `text::splitlines`.
  - Keys can differ from Python-built keys when a token holds a line break, so a verdict file is never mixed across builds (parity ledger, Diff row).
  - Each key is computed once per file (finding C15).
- **`rank(candidates, repro_valid) -> VoteReport`** (`src/agentless_mcp/core/vote.py:171-201`):
  - tiers are an enum;
  - clusters sort by (-size, minimum index);
  - presentation moves to `render` (finding C11).
- **Tests**:
  - Unit tables for each parser.
  - Property tests: `apply(diff(a, b), a) == b`; `parse(render(x)) == x` for blocks and diffs.
  - The vote fixtures from `tests/unit/test_vote.py`, as data.

### 2. `io` and `git`: writes, sandboxes, and test runs

- **`Fs::write_atomic(path, bytes)`**: write a temporary file in the same directory, fsync it, then rename it into place.
- **`Txn`**: stages writes into the target repository and commits them all or none. It rolls back on `Drop` unless committed. It replaces `_write_all` (`src/agentless_mcp/application/patch_service.py:543-604`, cc 17) and its stage and discard helpers.
- **Writes that are not repository edits** keep their own owners and are not transactional: the store's SQLite file (`store`, under the file lock), the hook log append and the lock file (`io`), the test-run capture files (`io`), and worktrees (`git`, through the git process). P0 D2's `disallowed-methods` keeps every direct file write inside `io` and every SQLite open inside `store`.
- **`GitWrite::worktree(top, at) -> Worktree`**:
  - The path is `<scratch>/worktrees/wt-<hex>`.
  - It adds `core.hooksPath=/dev/null` and refuses a scratch directory that resolves inside the target.
  - `Drop` removes the worktree and prunes it.
  - `diff` is `git diff` in the worktree.
  - Python source: `src/agentless_mcp/core/sandbox.py:246-372`.
- **`run_test(runner, cwd, command, limits, passthrough) -> RunResult`**: on the one `Runner`, in a process group. It keeps:
  - the environment allow-lists;
  - the statuses passed, failed, timeout, and error;
  - the `[... N earlier bytes dropped ...]` tail marker;
  - a 5 s TERM grace, then a 1 s KILL reap.
  - Python source: `sandbox.py:374-660`.
- **Tests**:
  - Write failure through the fake `Fs`: a staged second file fails, and the first is rolled back.
  - Rollback when the `Txn` is dropped without a commit.
  - Worktree cleanup when the test panics inside a `Worktree`.
  - Process-tree cancellation: a child that starts a grandchild is killed as a group within the grace period. The test uses the injected clock for the grace and asserts only that the tree is gone.

### 3. `patchcheck`: the seven patch checks

- Each check implements `checks::Check` with `Needs::Patch`. `Inputs` builds the repository facts and the parsed fragments only when a patch check is selected (finding C14).
- **The checks**:
  - **`undeclared_imports`** (`patchlint.py:906-1080`, cc 14, split): manifests are parsed by serde per format.
  - **`shadowing`** (`patchlint.py:1081-1149`).
  - **`near_duplicates`** (`patchlint.py:1150-1260`): keys come from the measured index (`metrics` body keys), so it does not re-parse every function (finding C7). The key change shows in the patch parity fixtures, not in the lint golden, which has no `near_duplicates` row. Write the prediction first.
  - **`dangling_references`** (`patchlint.py:1665-1850`).
  - **`dangling_callers`** (`patchlint.py:1851-1935`).
  - **`arity`** (`patchlint.py:1936-2040`): the call shape and parameter shape come from parse nodes. That replaces the hand-written lexer (`patchlint.py:1302-1650`, finding C3). The conservative refusals stay: a splat, a keyword splat, or a truncated signature gives no finding.
  - **`cycle_delta`** (`patchlint.py:2041-2099`, cc 12, split): `graph::import_cycles` on the before and after graphs.
- **One `Finding` type and one `not_checked` constructor** (finding C5). The finding sort is (check, path, line, message, evidence).
- **Tests**: a table of (repository fixture, candidate) to findings per check. Arity covers each conservative-refusal case.

### 4. `app`: write operations

- **`lint --candidates FILE`**: all ten checks. Pair checks get their pairs from the applied candidates. Candidates that do not parse, or that hold no edits, give one `not_checked` row, from one rule shared with validate (finding C12).
- **`patch parse`, `patch check`, `patch apply`, `patch normalize`** (`patch_service.py:217-465`): `_canonical` is one function (finding C12). `normalize` computes each key once (finding C15).
- **`validate`** (`src/agentless_mcp/application/validate_service.py:591-908`):
  - the baseline run, then each candidate applied once in its own worktree, the reproduction test, and the regression run;
  - `ValidateReport.warnings` (cc 12) and `_baseline` (cc 12) are split.
- **Verdict files**:
  - serde records with `deny_unknown_fields`, because the project owns this format;
  - domain checks after deserialization: record order, unique ids, and unique indexes (Codex V24);
  - one `From<&CandidateVerdict> for VoteCandidate` (finding C2);
  - the execution fingerprint uses the `json::JsonForm::Compact` form (`validate_service.py:1203-1209`);
  - the Rust build writes `"producer": "agentless-mcp <version> (rust)"` in the `run` record (`validate_service.py:430-442`). The reader refuses a `run` record without a Rust `producer`, with a message to re-run validate. A Python-built file has no `producer` field, so it is refused.
- **`vote`**: `patch::rank`, rendered by `render`.
- **Tests**: every operation on fake ports. `validate` uses a fake `Runner` that scripts the test outcomes.

### 5. `render` and `cli`

- Lint text and JSON gain the seven patch-check rows. Patch, validate, and vote reports are rendered, including the vote text (`vote.py:202-244`).
- The CLI write subcommands are generated from `OPS`. They keep exit codes 0, 1, 2, and 3, and the validate exit rules from `src/agentless_mcp/adapters/cli/main.py:1522-1601`.
- **Tests**: trycmd snapshots of every `--help` and the validate exits.

## Parity harness

- The lint golden (`tests/characterization/goldens/lint/repo_py.lint.json` and `.txt`) runs end to end through `lint --candidates`. It has rows for `arity`, `dangling_callers`, `dangling_references`, and `undeclared_imports`, and none for `near_duplicates`, `shadowing`, or `cycle_delta`.
- **Patch parity fixtures**: `scripts/patch_parity_fixtures.py` runs once under the frozen 0.10.1 build over a candidate corpus that makes each of the seven checks report at least one row, including each `arity` refusal case. It writes the rows to `crates/patchcheck/fixtures/`, and a Rust test compares them exactly.
- `near_duplicates` keys change by design. Write the predicted fixture diff first and check the actual diff against it.
- The patch subprocess tests (`tests/unit/test_cli.py:1563-1894`) and the validate and vote subprocess tests (1895-2113) run with the Rust driver.

## Exit gates

- [ ] The lint golden is byte-identical.
- [ ] The patch parity fixtures match for all seven checks, apart from the predicted `near_duplicates` change.
- [ ] The patch, validate, and vote subprocess tests pass against the Rust binary.
- [ ] Write failure, rollback, and worktree cleanup pass through the ports, and process-tree cancellation passes for test runs.
- [ ] Clippy passes with P0 D2's `disallowed-methods`, so no crate but `io` writes a file directly and no crate but `store` opens SQLite. The gate self-test (P0 D7, violation 9) proves the rule can fail.
- [ ] Layers, public API, suppressions, lint tables, clippy, and the erosion gate (the `rust-p2` build) pass.

## Risks

| Risk | Response |
|---|---|
| The arity check from parse nodes refuses less than the lexer did | Keep every conservative refusal as a named test case before porting. |
| A user votes with a verdict file written by the Python build | The fingerprint and keys can differ. The reader refuses a `run` record without a Rust `producer` field, with a message to re-run validate. |
| A sandbox process outlives the run on macOS | Process groups behave the same on Linux and macOS. The cancellation test runs on both CI rows. |
