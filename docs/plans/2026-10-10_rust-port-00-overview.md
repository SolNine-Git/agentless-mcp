# Rust Port of agentless-mcp: Overview

**Date**: 2026-10-10 | **Status**: approved design; P0a merged (PR #60); P0b and P0d committed on `feat/rust-port`; P0c next | **Design page**: [agentless-rs Blueprint](https://claude.ai/artifact/Qo75kbiyP12HhMN1E17Nsp) (revision 4, private) | **Phase plans**: [P0](2026-10-10_rust-port-p0-groundwork.md), [P1](2026-10-10_rust-port-p1-per-write-path.md), [P2](2026-10-10_rust-port-p2-read-surface.md), [P3](2026-10-10_rust-port-p3-write-surface.md), [P4](2026-10-10_rust-port-p4-mcp-and-retirement.md)

## Problem Statement

The erosion checks on `feat/erosion-checks` run on every Write and Edit through a Claude Code hook. At `1dc4d70`, one hook call on a 2,225-line file in mcp-local takes 327 ms with a fresh tag cache, 407 ms when one file in ten changed after the index, and 240 ms with no cache, where `clone_delta` does not run (medians of 30 runs). The hook process alone takes 34 ms. Interpreter start-up, imports, and Python tree walks take most of the rest. The parse itself is C and takes 7 ms for both sides.

The owner chose to rewrite every layer, the MCP server included, in Rust as one binary. The rewrite also removes duplicated knowledge: 30% of the Python source lines are prose, per-language facts live in six lists and five tables, each operation is declared three times, and 79 serializers are hand-written.

## Decisions (owner, 2026-10-10)

| # | Question | Decision |
|---|---|---|
| 1 | Rewrite, or Python latency steps first | Rewrite in Rust. Go was measured and rejected: each tree-sitter node access is a cgo call. |
| 2 | v1 MCP surface | Not ported. The last Python release marks it deprecated and names the first Rust release as the removal. The Rust server refuses `--surface v1` at start-up. |
| 3 | The four defects the goldens pin | Fixed in Python first, each with a predicted golden diff. |
| 4 | Grammars | All 22 linked into the binary. A new language comes in a release as one table row, its grammar, and its queries. A run-time path waits until users ask for grammars the project will not ship. |
| 5 | Windows | Not supported for now. Linux and macOS, as CI runs today. |
| 6 | Clone pairing | The `similar` crate's `Algorithm::Histogram`, pinned to an exact version. The lint golden's one `clone_delta` row runs through the matcher (`erosion.py:787`, keyed at 857), and it is matcher-invariant: rebuilt with `SequenceMatcher` (autojunk on and off) and with `git diff --diff-algorithm=histogram`, the lint goldens are byte-identical (measured 2026-10-10). Python keeps `autojunk` on, and Histogram has no such rule (P1 risks). |
| 7 | Community hash and pass count | Deleted. An unconverged grouping is shown as a one-line note. |
| 8 | Test and fixture paths | Fixtures (and `testdata`) skipped by every check. Tests skipped by `health`, `clone_delta`, and `contract_drift`. Tests kept for `complexity_delta`. |
| 9 | The cache the Rust hook reads in P1 to P3 | The Python `tags.db` (schema 17, which 0.10.1 writes), read-only. A missing file or another schema version gives the `no_tag_cache` gap, as the Python hook form does. The Rust store's own file arrives in P2 for the Rust CLI, and the hook moves to it in P4, when the Rust MCP server fills it. |
| 10 | Version of the Rust release | `0.11.0-rc.1` in Cargo, which maturin publishes as the PyPI pre-release `0.11.0rc1`. Plain installs skip a pre-release, so they stay on Python 0.10.1 while the deployed surfaces opt in. `0.11.0` follows when the P4 gates pass and the trial is clean. `1.0.0` waits for more validation. |

## Architecture

One Cargo workspace in this repository, under `crates/`, beside `src/agentless_mcp` until the Python tree is deleted after P4. Nineteen crates in nine layers. A crate depends only on crates in lower layers. Cargo refuses cycles, and a layer test (P0) refuses every other upward edge.

| Layer | Crate | Job | Depends on | Built in |
|---|---|---|---|---|
| L8 | `agentless` | Binary. Reads argv and the environment once, builds `app::Services`, picks a door. | cli, mcp, hook, app, store, git, io | P1 (mcp joins in P4) |
| L7 | `cli` | Command line generated from the operation table; exit codes. | app, render, prompts | P1, grows in P2, P3 |
| L7 | `mcp` | MCP server on rmcp, stdio and streamable HTTP. | app, render, prompts | P4 |
| L7 | `hook` | Claude Code hooks in process. | app, render, prompts, io | P1 (erosion gate), P4 (search gate) |
| L6 | `render` | Text output and the envelope; JSON through the Python-compatible formatter. | app, prompts, pycompat, kernel | P1, grows in P2 |
| L5 | `app` | One module per operation, the operation table, `Services`, `Session`. | patchcheck, checks, graph, store, git, io, facts, metrics, patch, prompts, kernel | P1, grows in P2, P3 |
| L4 | `patchcheck` | The seven patch checks. | checks, facts, graph, patch, metrics, syntax, kernel | P3 |
| L3 | `checks` | Check trait, one Finding type, cascade matcher, path-scope rule, the three pair checks. | metrics, syntax, prompts, kernel | P1 |
| L3 | `graph` | Name resolution, tiers, file and symbol graphs, ranking, communities, cycles, paths. | facts, pycompat, kernel | P2 |
| L3 | `store` | Until P4, a read-only reader of the Python `tags.db` for the hook (decision 9). From P2, the Rust per-file cache under a complete artifact key, with pure freshness, plan, and eviction. | facts, metrics, io, kernel | P1 (`tags.db` reader), P2 (own store), P4 (reader deleted) |
| L2 | `facts` | Symbols, imports, references; skeleton, slices, locations. | syntax, kernel | P2 |
| L2 | `metrics` | Units, complexity, lines, body keys, clone blocks and keys. | syntax, pycompat, kernel | P1 |
| L2 | `patch` | SEARCH/REPLACE and unified diff parsing, apply, vote. | syntax, kernel | P3 |
| L2 | `git` | Git queries as typed values through the one runner. | io, kernel | P1 (`log_span` in P2, worktrees in P3) |
| L1 | `syntax` | Language table, static grammars, parse, flat index, queries. The only crate with `unsafe`. | pycompat, kernel | P1, grows in P2 |
| L1 | `io` | Process runner, bounded walk/read/write, lock, deadline, config read once. The only crate that writes files. | kernel | P1 (write port in P3) |
| L1 | `prompts` | Caller-facing strings from JSON; one typed function per message. | kernel | P1 |
| L0 | `pycompat` | Every Python behavior the goldens need, one function each. | none | P1 |
| L0 | `kernel` | Error enum, ids, spans, keys, `Bounded<T>`, `Limit<MIN, MAX, DEF>`. | none | P0 |

## Phase Map

| Phase | Delivers | Exit gate (summary) | Est. Rust lines |
|---|---|---|---|
| [P0](2026-10-10_rust-port-p0-groundwork.md) | Python fixes and a Python release; the workspace, `kernel`, and every gate with a negative fixture | Each gate fails on its own negative fixture; Python release published | about 600 |
| [P1](2026-10-10_rust-port-p1-per-write-path.md) | The per-write path: pair checks, lint, capabilities, the erosion hook on the `tags.db` reader | Hook latency in each cache state (thresholds below); outcome suite; tokenizer corpus; complexity goldens; pair-check parity | about 8,000 |
| [P2](2026-10-10_rust-port-p2-read-surface.md) | The read surface: map, symbols, refs, read, health, graph views, history; the Rust store and `index` | 49 read goldens; Loc-Bench ranked_files 0 of 150 differ; surface diff with no unexplained loss | about 7,300 |
| [P3](2026-10-10_rust-port-p3-write-surface.md) | The write surface: patch checks, candidates, normalize, validate, vote | Lint golden and patch parity fixtures; write, rollback, and cancellation tests | about 3,500 |
| [P4](2026-10-10_rust-port-p4-mcp-and-retirement.md) | The MCP door, the search gate, wheels, release `0.11.0rc1` then `0.11.0`; then the Python tree deleted in a follow-up | tools/list equals Python v2; installed-wheel sessions; AGENTS.md release | about 800 |

Line figures are judgment estimates and were not re-derived when the store writer moved from P1 to P2. P1 gives the first measured check on them.

The proposed P1 latency thresholds are p50 50 ms or less and p95 100 ms or less on mcp-local, in each cache state. The owner confirms them before P1 starts.

## Rules for Every Phase

1. **One branch, `feat/rust-port`, with one commit per plan part** (owner, 2026-10-10). `/pr-review` runs on each commit that adds Rust code, scoped to that commit alone. The branch merges to `main` by pull request at each release and continues afterwards. The Python deletion follows P4 in its own pull request. Each phase ends in a release. P1 to P3 end in a merge commit that the owner tags `rust-pN`, with no PyPI upload; P4 ships the wheel.
2. **The gate is a pinned build, never the build under test.** Python 0.10.1 gates until P1 ends. CI runs a locked checkout of its release commit. Until P0c publishes 0.10.1, that checkout is the P0a merge `9bb51ac`. The pre-push `cargo-erosion` hook runs whatever the uv tool install holds. It checks neither the version nor the source of that install, so CI is the only pinned gate. From then on, CI builds the Rust gate with `cargo install --locked --git <this repository> --rev <commit>` from the latest `rust-pN` commit and caches it by commit id. The `health` ratchet stays on Python 0.10.1 until P2, so `cargo xtask erosion` takes a lint binary and a health binary.
3. **Gates run in block mode on `crates/`:** `complexity_delta`, `clone_delta`, and `contract_drift` (with `boundary_globs` on the shell crates), `--fail-on warning,advisory,not_checked` (a `clone_delta` finding is `advisory`), the health ratchet, the layer test, public API snapshots, clippy limits, the static gates, the ground-truth checks, `cargo-deny`, and `cargo-machete`. The policy gate fails a change to a policy file, unless each commit that changes it carries a `Policy-Change: <path>` trailer and one of those commits made HEAD's value, so a merge cannot bring a value from outside the range. The gate judges HEAD on every run, and the working tree fails when it differs from both HEAD and the reference (P0 "P0d as built").
4. **No suppressions.** `cargo xtask suppressions` fails on each `allow`, `expect`, `warn`, or `ignore` attribute and each `rustfmt::` or `clippy::` tool attribute that its token scan finds in `crates/` or `xtask/`, inner or outer, inside `cfg_attr` too, unless `crates/suppressions.toml` lists it. Each entry names the file, the lint, the count, and the reason, and the owner approves it in review. The scan reads source text, not macro output. So the static gates also refuse the forms that P0 "P0d as built" lists, each of which could build an attribute or hide code from clippy. `cargo xtask ground-truth` asks the compiler instead of the tokens. clippy runs with each warn and deny lint force-warned, and each diagnostic must match a ledger entry for its target. rustc must read only the scanned sources. The workspace forbids each cfg that rustc and the package features do not declare. The workspace sets `allow_attributes`, `dbg_macro`, `todo`, and `unimplemented` to `forbid`, so clippy rejects a suppression of those four, a macro-built one too (E0453). rustc alone accepts it. Neither the token rules nor the ground-truth checks are complete. P1 "Accepted residuals" lists the residuals, for example lints in external macro output (R-7), the cfg names that rustc knows (R-8), and a change that rewrites xtask itself (R-27). P1 "Port constraints" lists each rule with its alternative. Clippy's `allow_attributes` catches only an outer `#[allow]`, so it is not the gate. Fix the code a lint points at.
5. **Comments**: one-line contract docs on public items; a why-comment of 1-2 lines; no provenance in code. Measurements go in commit messages and CHANGELOG.md.
6. **Goldens change only after a written prediction.** Rust tests compare output with the golden files byte for byte and print a diff on failure. The insta crate is used only for snapshots of typed values, and CI runs `cargo insta test --check`. `cargo xtask goldens --update` rewrites the golden files. A golden diff that was not predicted stops the work.
7. **Dual maintenance.** From P0 until the Python tree is deleted, the Python tree takes defect fixes only. Each fix lands in both trees, with a test in both. No Python change may bump the `tags.db` schema without a matching change to the Rust reader (decision 9).
8. **Parity questions go to the ledger.** A Python behavior that a golden or a consumer depends on gets one owner in one crate (see the parity ledger in [the design records](../analysis/2026-10-10_rust-port-design-records.md)) and a fixture made by the pinned CPython 3.13 at development time. No test runs Python.
9. **Hermetic tests.** No listening ports, no real clock, no ambient environment, no shared state. `cargo nextest` with a per-test timeout.
10. **Benchmarks follow docs/analysis/benchmark-methodology.md.** Never against the shared working tree; a frozen copy or a detached worktree only.
11. **Mutation score.** From P1, a nightly `cargo mutants` job runs on the pure crates (`kernel`, `pycompat`, `metrics`, `checks`, then `facts`, `graph`, and `patch` as they arrive). Its first run sets the floor, and the floor only rises.

## Grammar sources (D1, measured 2026-10-10)

The Rust build vendors the 22 grammars' C sources from the language pack's release asset `parser-sources-1.14.3.tar.zst` (sha256 `f13bcf0be1b9a99c45ead43671ae311346c2a10807c3fdc7c76407445ea7f2c7`), not from upstream at the manifest commits: for 11 grammars the pack's CI regenerated `parser.c` at ABI 14, and upstream's committed file gives different trees on inputs with syntax errors. The scanners compile with the pack's `ctype_shim.h` and utf8proc 2.10 from tag v1.14.3, and the runtime is pinned to `tree-sitter = "=0.26.9"`, the core that py-tree-sitter 0.26.0 bundles. A test build on that set matched the Python bundle on 7,558 of 7,558 parses. A sha256 manifest of every vendored file gets a CI check. The Python pin does not move. [The full report](../analysis/2026-10-10_rust-port-grammar-pin.md) holds the evidence.

| Language | Upstream | Revision | Source | ABI | License |
|---|---|---|---|---|---|
| bash | tree-sitter/tree-sitter-bash | `a06c2e4415e9` | regen | 14 | MIT |
| c | tree-sitter/tree-sitter-c | `b780e47fc780` | regen | 14 | MIT |
| cpp | tree-sitter/tree-sitter-cpp | `8b5b49eb196b` | exempt | 15 | MIT |
| go | tree-sitter/tree-sitter-go | `2346a3ab1bb3` | regen | 14 | MIT |
| java | tree-sitter/tree-sitter-java | `e10607b45ff7` | committed | 14 | MIT |
| javascript | tree-sitter/tree-sitter-javascript | `58404d8cf191` | regen | 14 | MIT |
| lua | MunifTanjim/tree-sitter-lua | `4fbec840c341` | committed | 14 | MIT (LICENSE.md) |
| python | tree-sitter/tree-sitter-python | `26855eabccb1` | regen | 14 | MIT |
| ruby | tree-sitter/tree-sitter-ruby | `ad907a69da0c` | committed | 14 | MIT |
| rust | tree-sitter/tree-sitter-rust | `77a3747266f4` | regen | 14 | MIT |
| tsx | tree-sitter/tree-sitter-typescript | `75b3874edb2d` | committed | 14 | MIT |
| typescript | tree-sitter/tree-sitter-typescript | `75b3874edb2d` | committed | 14 | MIT |
| csharp (file c_sharp) | tree-sitter/tree-sitter-c-sharp | `9150f7d56bb4` | exempt | 15 | MIT |
| hcl | MichaHoffmann/tree-sitter-hcl | `64ad62785d44` | regen | 14 | Apache-2.0 |
| json | tree-sitter/tree-sitter-json | `001c28d7a298` | committed | 14 | MIT |
| kotlin | fwcd/tree-sitter-kotlin | `c8ac3d262724` | committed | 14 | MIT |
| php | tree-sitter/tree-sitter-php | `3fda2fb95771` | regen | 14 | MIT |
| scala | tree-sitter/tree-sitter-scala | `b4464dce74d8` | exempt | 15 | MIT |
| sql | DerekStride/tree-sitter-sql | `c2e1e08db1ea` | regen | 14 | MIT |
| swift | alex-pinkus/tree-sitter-swift | `28fe3a8a8558` | regen | 14 | MIT |
| toml | tree-sitter-grammars/tree-sitter-toml | `64b56832c2cf` | committed | 14 | MIT |
| yaml | tree-sitter-grammars/tree-sitter-yaml | `a1c4812a73ec` | regen | 14 | MIT |

## Baselines (measured 2026-10-10)

The hook figures come from `scripts/hook_latency.py --build 1dc4d70 --build 0037919` (D8), on 32 CPUs, Linux 7.2.8, and system Python 3.13.11. Each build is a `git archive` with `uv sync --frozen` and its own `XDG_CACHE_HOME`. The target is a clone of mcp-local at `f17af96`, and the payload is a Write of `src/readers/document_readers.py` (2,225 lines) with one 4-line function appended. Each figure is 30 runs after one warm-up run, with the builds taking turns. The p95 is the nearest rank. The script checks each state: `clone_delta` is off by `no_tag_cache` only when no cache exists, and the cache rows do not change during the runs. P0c runs the script again on `v0.10.1`, and P1 scores its latency gate with it.

| Figure | Value (median / p95) | Commit |
|---|---|---|
| Erosion hook, no tag cache (`clone_delta` gives `no_tag_cache`) | 233 / 242 ms; 236 / 242 ms | `1dc4d70`; `0037919` |
| Erosion hook, fresh tag cache | 317 / 328 ms; 317 / 326 ms | `1dc4d70`; `0037919` |
| Erosion hook, tag cache stale for 19 of 183 files | 397 / 405 ms; 394 / 402 ms | `1dc4d70`; `0037919` |
| Hook process with the gate off | 34 / 38 ms; 34 / 37 ms | `1dc4d70`; `0037919` |
| `agentless-mcp index` on that mcp-local clone | 3.6 s, 136 MB maximum RSS | `1dc4d70` |
| Rust prototype, one edit both sides | 13.5 ms | prototype, parse and measure only (deleted) |
| This repository: units, over threshold, erosion, clone share | 1,677, 55, 0.191, 0.034 | `1b17fb2`, `agentless-mcp health --repo .` |
| Source lines: code / docstring / comment / blank | 23,800 / 9,429 / 2,810 / 6,011 | `1b17fb2`, ast and tokenize count |
| Test suite | 4,907 passed in 77.3 s | `1b17fb2`, `uv run pytest` |
| 22 grammars linked into one test binary | 47.3 MB | measured by the grammar research agent |

## Evidence

[The design records](../analysis/2026-10-10_rust-port-design-records.md), copied from the design page, hold the 79 review findings (ids A1-F15, S1-S2), the Codex review findings (V28-V47 one by one, V1-V27 as one row) and their resolutions, the parity ledger, and the alternatives that were measured and rejected. The phase plans cite those ids. The records' line citations are at `1b17fb2`. The plan validation of 2026-10-10 (a Claude `validate` run at `1dc4d70`, each finding re-checked against the code) produced decision 9 and the revisions in each phase plan. A second validation the same day (at `7c816e3`) corrected decision 6, two citations, the P0 status, and P0d (D8).

Every Python line citation in these plans is at `1dc4d70` unless it names another commit. Read one with `git show 1dc4d70:<path>`: P0a and P0b moved the code since.
