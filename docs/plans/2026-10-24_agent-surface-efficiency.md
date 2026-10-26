# Plan: a cheaper, sharper agent surface

**Date**: 2026-10-24 | **Status**: revision 4. Paused after C3 and TR landed; see Status (2026-10-25) | **Evidence**: [benchmark-methodology.md](../analysis/benchmark-methodology.md), the 2026-10-24 entries

## Status (2026-10-25)

Paused by the owner after C3 and TR landed, to save cost. The confirmation below runs only when the owner says so.

### Landed and kept

| Change | Commit | Measurement |
|---|---|---|
| C1 | 14c431b | flat60 screen: read errors 45 to 0, turns -13%, context tokens -17% (both CIs exclude 0) |
| C2, C2b, C4, C5, D, D2 | c103f63 to ea6ed16 | 300-instance confirmation, forced arm, 14c431b against ea6ed16: context per session -9.5% and answer bytes -10.2% (both CIs exclude 0), quality 0 of 21 metrics changed, WCC -0.0055 [-0.0147, +0.0035]; first-turn context -1,873 tokens from the C2, C2b and C4 step |
| Correctness batch (OV, CB1-CB5, D3, CI1) | d7c0a70 to f6105a2 | flat60 screen: quality 0 of 21, WCC +0.0026 |
| U-a, usage files in `find` | 8d0dbba, 6388ff5 | flat60 screen: quality 0 of 21, WCC -0.0117 [-0.0316, +0.0076]; named gold files 111 to 117 (11 gained, 5 lost, 8 of the gains through a usage block) |
| Follow-up batch FB1-FB4 | bc86507, 854c90c, 87e8c6e, edea4a4 | flat60 screen: quality 0 of 21, WCC -0.0044 [-0.0273, +0.0186], first-turn +0; Loc-Bench retrieval tier 0 of 150 ranked lists changed (FB3, FB1). FB2 removes 40,162 declaration rows from lucene refs answers; FB2 makes refs of a name with many definitions about 16% slower on lucene (measure first) |
| C3, progressive disclosure | 542f481 | Not screened (owner, cost). tools/list with one root 10,534 to 10,403 bytes; the always-loaded set 9,565 to 5,516 bytes (1,881 to 1,106 cl100k). Derived first-turn change: about -99 tokens with tool search off (the bench as configured), about -1,280 with tool search on |
| TR, target resolution and honest absence | ef6a1a2 | Replay of 523 recorded calls: at the base, 65 of 251 refs answers were false zeros (18 unprefixed `path::name`, 47 undeclared names). After: 20 bind, 53 say that no symbol matches (exit 1, isError), 149 gain a line that splits binding rows from candidates; 0 unclassified changes. Always-loaded set +23 bytes |

Not kept: U-b (several names per `find` call). Its screen showed 0 of 110 `find` calls with a list.

Lesson: a flat60 placebo (the same build in two windows) moved 5 of 21 metrics. A 60-instance screen shows mechanism and gross changes only. Quality decisions wait for the confirmation.

### Left

1. **The confirmation**, when the owner says so: 300 instances, four arms (forced, hooked, ungated `claude_code_agentless`, native baseline), the new build against ea6ed16. The bench as configured runs with tool search off. swe-explore-bench now has an opt-in `BENCH_TOOL_SEARCH=1` that adds ToolSearch to `--tools` (uncommitted there), which C3 needs to show its effect.
2. A review of the branch, then the merge.
3. C6 to C10 are not started.
4. The owner's review `docs/analysis/2026-10-25_tool-surface-usefulness-review.md` raised six defects. TR fixed defect 1 and the headline part of defect 3, and C3 states that `path` ignores edge direction (defect 4). Defect 2 (no Rust cross-crate binding tier), defect 5 (cross-language candidate edges), defect 6 (a union schema per tool) and its gate items (log fields, an ungated arm, enrichment) are open.
5. Owner actions outside the repository after C3: add `mcp__agentless__graph` to `permissions.allow`, and update the skill files that call `orient` with a moved operation.

## Problem

The isolated 300-instance run (Rust 0.11.0rc1, claude-sonnet-5-5) found three costs and one gap:

- **The surface costs about 5,500 context tokens per turn.** First-turn context, median: baseline 5,518, hooked 11,010, forced 9,522. The task prompts differ by about 150 tokens.
- **Responses carry overhead.** Over 7.1 million characters of agentless answers, the two receipt lines are 6.0% of all bytes, 19.0% of a `symbols find` answer and 25.6% of a `find_referencing_symbols` answer. Stable ids are 31% of a `find` answer.
- **One input shape failed a third of the `read` calls.** C1 fixed it.
- **The gap: the agentless arms name fewer files.** 2.0 to 2.2 files per task against 2.38, so they miss more gold files (hit-file rate -0.019 hooked, -0.038 forced). WCC does not differ from the baseline.

## Where the gold files are lost

A classifier over the 600 agentless sessions and the 300 baseline sessions (`/var/tmp/claude-rust-port/suff/`) asked, for each gold file the baseline named and an agentless arm missed, whether any tool answer showed it.

| Arm | Missed | Never shown | Shown, not named |
|---|---|---|---|
| forced | 96 | 79 | 17 |
| hooked | 65 | 49 | 16 |

An agent names 99% of the gold files whose content it reads, in every arm. The loss is in what the answers show. The causes, over the 161 missed files:

1. **Usage sites (66).** The gold file uses a name the agent searched for and does not declare it: a test that calls the function, a module that reads a setting. Grep returns usage sites; `symbols find` returns declarations only.
2. **Several names in one search (37).** The baseline's Grep matched two or more names at once in 24 of these 37 cases; `find` takes one name per call.
3. **A path the model already knew (18).** No tool change reaches these.
4. **Listed, never opened (29).** Map rows 16, `find` or refs rows 7, other rows 6.

Also found: 37% of the hooked arm's map rows are files that recur in the maps of 3 or more tasks of the same repository and are not gold (derived, a proxy for hubs), and the map's test-file cap hid the test with the source file's name in 4 cases.

## What the agent uses

600 agentless sessions (hooked and forced, bench300):

| Tool | Operation | Calls |
|---|---|---|
| read | slice | 1,002 |
| symbols | find | 676 |
| orient | map | 300 |
| symbols | overview | 50 |
| find_referencing_symbols | | 39 |
| symbols | expand | 36 |
| read | dir | 18 |
| symbols | locate | 4 |
| symbols | explain | 1 |
| orient | communities, cycles, diagram, path, health | 0 |
| history, capabilities | | 0 |

These are bug-localization sessions. Other work (review, refactoring, the owner's own sessions) uses `health`, `path`, `history` and `find_referencing_symbols`, so a change that hides a tool must keep it reachable.

## Other tools (survey, 2026-10-24)

Measured offline on the caddy, tokio and matplotlib snapshots (`/var/tmp/claude-rust-port/survey/`):

| | agentless 0.11.0rc1 | CodeGraph 1.6.2 | graphify 0.9.95 |
|---|---|---|---|
| Cold index, peak memory | 0.09-1.29 s, 33-140 MB | 1.2-6.3 s, 525-1,669 MB | 3.8-26.6 s, up to 702 MB |
| Warm query | 12-304 ms | 0.4-4 ms | 0.3-8 ms |
| A renamed function shows | on the first call (0.02 s) | after 0.78 s (watcher) | after a manual rebuild |
| tokio `is_empty` definitions | 20 of 38 in path order, 0 gold files | all 32, 4 gold files, 2.2 KB | 25, 4 gold files |
| Quality evidence | 300 instances, paired CIs, placebo | 7 questions, cost metrics only | 6 questions, LLM judge |

agentless pays its query time for freshness: each call checks the tree. Its weak point is answer shape: exact hits in path order, method callers as unranked name lists, data-file keys in `find`, and no text search.

## The changes

One change, or one coupled group, at a time. Each is built, gated, screened, and kept only if its measurement holds.

| # | Change | Coupled because | Instrument |
|---|---|---|---|
| C1 | `read` accepts a flat `[start, end]` pair. Built: 14c431b | | screened: errors 45 to 0, turns -13%, context tokens -17% |
| C2 | Schema rewrite: shorter descriptions, `operation` as an enum generated from the operation table, optional parameters without the `anyOf [T, null]` wrapper. Built: c103f63, with C2b (78f5022), which restored three routing sentences | all three change only how the contract is written | screened, kept: see "C2 result" |
| C4 | Output diet: a one-line receipt (`repo:` only when more than one root is selectable); a card's line 2 without the language, and without the owner when the qualified name spells it; whole ids on refs rows. Map and overview keep their pattern line. Built: 7ce5c6e | both change answer bytes, not answer content | replay: -5.0% of answer bytes (find -21%, refs -22%, dir -23%), 200 of 200 answers equal after the predicted transforms; then the screen |
| C5 | Clear refusals: one actionable sentence per bad parameter instead of the pydantic dump, including the union refusal C1 made longer. Real errors keep `isError`: on bench300, 82 of the 89 hooked sessions with an agentless error kept calling agentless | C3 needs it: a moved operation must get a refusal that names its new tool | refusal fixtures, retry counts in the screen |
| D | Data files as an outline: a JSON, YAML or TOML file stores its keys to depth 2 with type and item count, capped per file; scalar values get a preview of about 40 characters; objects and lists show no values. `find` ranks data keys after code symbols and collapses them per file; `overview` of a data file prints the outline. CodeGraph indexes no JSON keys; graphify indexes config JSON only, capped at 1 MiB, 500 pairs and depth 6 | one decision: what a data file contributes | index rows, database bytes, index time and peak memory on this repository and the #56 repro; the find and overview surface diff; then the screen |
| U | Usage sites in `find`: after the declarations, `find` lists the files that use the name (from the identifier index, not full fan-in), with a count and a cap; `find` accepts several names in one call. Targets causes 1 and 2 above (103 of 161 missed files, derived) | both answer the question an identifier Grep answers | the surface diff, the classifier rerun, then the screen and the 296-instance confirmation |
| C3 | Progressive disclosure: `orient` keeps `operation` (enum `map`, still required, because the gate's refusal tells agents to pass `operation="map"`); communities, cycles, diagram, path and health move to a new tool without the `anthropic/alwaysLoad` hint; `find_referencing_symbols` and `capabilities` lose the hint; `no_cache` leaves the MCP surface; `repo_root` is published only when the server has more than one root. CodeGraph lists one tool by default and keeps the rest callable | one decision: what loads every turn and what is reachable on request | first-turn context, ToolSearch round trips, the screen, then the confirmation |
| C6 | Relevance of symbol answers: every exact hit as one compact line before any substring hit; same-rank hits by proximity to the session's focus, not path order; substring hits ranked after exact hits, not capped away (a substring hit first showed 29 of the forced arm's 516 named gold files); a `path` scope for `find`; name-only-ambiguous tiers collapsed to a count plus the call-site text, same-directory candidates first. Later, separately: receiver-type inference for method callers, as CodeGraph does | all decide which candidates a symbol answer shows | the surface diff, then the screen |
| C7 | Map hub penalty: rank by the focus-personalized score relative to the global score, so central utility files stop filling the top 10; a test companion whose name matches a listed source file ranks first | | the Loc-Bench retrieval tier first (free), then the screen |
| C8a | Gate hint: when an allowed Grep searches for an identifier, the answer adds that `symbols find` gives the definition and its usage files. After U, because before U `find` returns no usage sites | | the hooked screen |
| C8b | Gate re-arm: after 3 native searches for identifier-like patterns, the gate asks for an agentless call again, at most once per 2 minutes, and an agentless call resets the count (Serena's hook uses 3 greps, 3 reads or 4 mixed calls and the same spacing). Literal-text searches stay allowed until C10 | follows C8a, so each effect is attributed | the hooked screen |
| C9 | Gate: refuse an unbounded Read of a large file until a `symbols overview` or a `read` slice of it | | the hooked screen |
| C10 | A text search with structure: literal or regex matches, each reported with its enclosing symbol and stable id, so a quoted error message lands on a function. All three graph tools in the survey failed on a literal error string | a new operation; needs its own spec (`/spec`) | spec first |

Why this order: C4 and C5 are the cheapest, and their size effect is measurable without a model run. C5 comes before C3, because the server validates arguments before it selects the operation (`crates/mcp/src/dispatch.rs:70`), so an old operation value reaches the validator first. D comes before C6 so that data keys do not distort C6's measurement. U comes before C6 and before C8, because it targets the largest measured loss and C8 depends on it. C6 and C7 change what answers contain, so they need the surface diff and the retrieval tier. C8 and C9 change agent behavior through the hook. C10 is a new capability and the largest piece.

## C2 result

Two screens, each against the old build in the same window, forced arm, 60 instances:

| | C2 (c103f63) | C2b (78f5022) |
|---|---|---|
| First-turn context, every session | -1,911 tokens | -1,873 tokens |
| Context tokens per session | -17.8% (CI excludes 0) | -3.4% (n.s.) |
| Turns | -0.9% | +6.0% (n.s.) |
| Sessions using native Read (old build: 3 and 4) | 21 | 13 |
| Calls to Grep, which the forced arm lacks | 3 | 0 |
| Tool errors (old build: 2 and 0) | 8 | 2 |
| Quality metrics with a CI that excludes 0 | 1 of 21 (wcc@200 -0.016) | 1 of 21 (optional_coverage +0.022) |

C2's shorter `read` and `lines` descriptions moved reads to native Read, and its orient sentence "Search text with Grep" made the agent call a tool the arm does not have. C2b restored the routing content of those three sentences. The placebo (the old build in two windows) moved context tokens -9.5% with a CI that excludes 0, so per-session context tokens at n=60 depend on the window; the first-turn context is the deterministic measure.

## Line numbers in relationship answers

The owner asked whether answers that carry relationships, not code, need line numbers. Measured: line numbers are 1.9% of a `find_referencing_symbols` answer and 3.5% of a `symbols find` answer. The receipt and the stable ids cost far more. The benchmark's answer is a list of files and line ranges, and the agent reads a call site by its line, so line numbers stay. C4 cuts the receipt and the id display instead.

## Measurement protocol

- **The screen**: `flat60.jsonl` (seed 20261024, 60 of the 296 instances under the walk bound). The old and new builds run in the same window: the forced arm for surface and output changes, the hooked arm for gate changes. Paired bootstrap 95% CI, 20,000 resamples, on the 21 quality metrics, turns, and context and output tokens, plus the tool mix and the tool errors by kind. The screen detects mechanism and large quality changes. It cannot certify that quality did not drop: at n=60 the WCC half-width is about 0.034, wider than the 0.019 noise floor. One metric in 21 with a CI that excludes 0 is the expected chance rate.
- **A placebo per window**: the unchanged build against its last run on the same 60 instances.
- **The confirmation**: the 296-instance set, which clears the about 192 instances a 0.019 WCC margin needs. It runs after C2, C4 and C5 together, and again after U and C3.
- **Deterministic measures** where they exist: tools/list bytes, first-turn context, answer bytes by replay of recorded calls, the classifier of where gold files are lost.
- **The release regression gate** in benchmark-methodology.md for every change that ranks, resolves or renders.

## Gates to add (light, fast, never stale)

- **Never stale, per answer**: a test that replays real commits of a fixture repository and checks that each cached tool answer equals the uncached answer. The property test `warm_equals_cold_over_random_edits_and_runs` proves it per file at the facts layer only. CodeGraph measured 5.7% wrong connections after an 80-commit replay of its incremental index.
- **Fast and light**: a release check on the three survey repositories: cold index time, peak memory, and warm latency per operation at the median and the 95th percentile, against the previous release. Today nothing enforces speed or memory.
- **Answer bytes**: C4's two-build replay of recorded calls, kept as a standing measure per operation.
- **Control arm**: every run checks that the baseline arm cannot reach agentless. bench300 passed: 300 of 300 baseline sessions had only Glob, Grep and Read, and none named agentless.

## Files each change touches (beyond its own code)

- **C2, C3**: the exact surface fixtures (`crates/app/src/surface/tests.rs`, `crates/app/fixtures/surface/surface.json`, `crates/mcp/src/tests/wire.rs`), the hook test that shares the unlock fixture (`crates/hook/src/search/tests.rs`), the agent guide's operation contract and eager-loading sections (`crates/app/data/agent-guide.md`), and the CHANGELOG.
- **C3**: the gate's unlock list (`crates/app/src/surface.rs` localizing table, `crates/hook`), which names `orient(map|path)`.
- **C4**: the receipt parsers (`crates/mcp/src/tests/calls.rs` masking, `crates/cli/tests/cli/scenarios.rs`, `scripts/surface_cmp.py`) and the guide's receipt, find-symbol and refs sections.
- **D**: the data-key extractor (`crates/facts/src/extract/data_languages.rs`), the language tables (`crates/syntax/languages/{json,yaml,toml}.toml`), the guide's data-file paragraph.
- **Any change to the orient description**: the token-band tests (`crates/app/src/tokens/tests.rs`), which check that orient publishes "a real BPE count runs 10-35% higher".

## Alignment with Anthropic's guidance

- "Simple tool descriptions" and "design interfaces" instead of examples (context engineering for Claude 5, 2026-07-24): C2's enums and short descriptions. The C2 screen showed the limit: a routing sentence and a parameter example carry tool choice, and C2b kept them.
- Progressive disclosure through tool search with deferred loading, and preconditions inside the tools that return an instruction the model can act on (when to fold subagents into the main loop, 2026-10-22): C3 and C5.
- Token-efficient responses and actionable errors (writing effective tools for agents): C4 and C5.
- "Measure, change one thing at a time, and keep what holds up" (2026-10-22): the protocol above.

## Risks

- **0.6.2 deferred every schema and bought nothing**, because the gate made the agent fetch each schema anyway. C3 defers only the tools a localization session rarely calls, and keeps the routing text that tells the agent they exist.
- **Shorter descriptions changed tool choice** (C2, measured). Every description change is screened with the tool mix, not the quality metrics alone.
- **Compression can cost recall.** The usage-site and multi-name losses are content an answer lacks, not bytes it carries. No change cuts answer content to save bytes.
- **C8b can cost coverage.** Withholding native search costs coverage (forced below hooked), and before U `find` returns no usage sites. C8b therefore follows U and applies only to identifier-like patterns.
