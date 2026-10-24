# Plan: a cheaper, sharper agent surface

**Date**: 2026-10-24 | **Status**: revision 2, after a prompt review by Claude, the local consult model and Codex gpt-6.1-sol. C1 is built and screened; nothing else is built | **Evidence**: [benchmark-methodology.md](../analysis/benchmark-methodology.md), the 2026-10-24 entries

## Problem

The isolated 300-instance run (Rust 0.11.0rc1, claude-sonnet-5-5) found three costs and one gap:

- **The surface costs about 5,500 context tokens per turn.** First-turn context, median: baseline 5,518, hooked 11,010, forced 9,522. The task prompts differ by about 150 tokens. This is an estimate of the schema cost from the first turn, not a per-turn measurement.
- **Responses carry overhead.** Over 7.1 million characters of agentless answers, the receipt header is 7.3%, line numbers 7.0% and stable ids 5.6%. In a `symbols find` answer, the receipt is 25% and the stable ids 31%.
- **One input shape failed a third of the `read` calls.** C1 fixes it.
- **The gap: the agentless arms give narrower answers.** They name 2.0 to 2.2 files per task against 2.38, so they miss more gold files (hit-file rate -0.019 hooked, -0.038 forced). WCC does not differ from the baseline.

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

## A hands-on comparison (Claude, 2026-10-24)

Three bench300 tasks where the forced arm lost gold files that the baseline found. Native means `rg` and a targeted read. agentless means the 0.11.0rc1 CLI, which renders what the MCP tools render.

| Task | Native | agentless |
|---|---|---|
| tokio-6603 (Rust): `is_empty` wrong after 32 messages | 3 calls, 1.6 KB: `rg 'fn is_empty' tokio/src/sync/mpsc` found 4 of 5 gold files; the body led to the fifth | `find is_empty`: 20 of 38 exact hits shown, 0 in mpsc (same-rank hits in path order, no path scope). `find Receiver.is_empty`: 3 hits, 0.6 KB, good. `map` on the two receiver files: 10.3 KB, misses list.rs and block.rs, includes a scheduler worker, a file test and a benchmark. `explain`: 30 outgoing and 170 incoming name-only-ambiguous candidates, 4.5 KB, the true callee not shown |
| matplotlib-24627 (Python): `cla()` keeps `.axes` on artists | 2 calls, 1.9 KB, 3 of 4 gold files | `find cla`: the 3 exact hits first, then 130 substring hits (`clabel`, `clamp`, every `__init_subclass__`), 20 shown. `map`: 17.4 KB, misses figure.py and container.py, the top 10 holds utility hubs (_api, cbook, transforms, colors, path) |
| caddy-6288 (Go): heredoc CEL expressions fail | 1 call, 126 bytes: the quoted error message found dispenser.go | agentless has no text search. `map` on the issue's terms: 9.2 KB, dispenser.go third, lexer.go and parse.go absent, hubs again |

From the agent's side: for a concrete issue with a distinctive identifier or an error string, scoped text search plus a targeted read is faster and cheaper. agentless is best at an exact qualified-name lookup, a file overview, a line slice with scope headers, and resolved callers where scope analysis exists (Python). Its noise has three sources: the map's preference for central utility files, unscoped and substring `find` results, and name-only-ambiguous edge lists for common method names.

## The changes

One change, or one coupled group, at a time. Each is built, gated, screened, and kept only if its measurement holds.

| # | Change | Coupled because | Instrument |
|---|---|---|---|
| C1 | `read` accepts a flat `[start, end]` pair. Built: 14c431b | | screened: errors 45 to 0, turns -13%, context tokens -17% (60-instance forced pair) |
| C2 | Schema rewrite: shorter descriptions, `operation` as an enum generated from the operation table in `crates/app/src/surface.rs`, optional parameters published without the `anyOf [T, null]` wrapper. Every tool, operation and parameter stays, and the server accepts the same inputs (null still reads as absent) | all three change only how the contract is written | tools/list bytes and first-turn context, then the screen |
| C4 | Output diet: the receipt shrinks to one line that keeps the repository, the freshness fields and the repository-data boundary; `find` and the fan-in lists print a shorter display form. The stable id that `expand` and `history` accept stays `<prefix>:<path>::<qualified name>` | both change answer bytes, not answer content | the surface diff over the Loc-Bench checkouts (bytes per answer, no lost content), then the screen |
| C5 | Clear refusals: one actionable sentence per bad parameter instead of the pydantic dump, including the union refusal C1 made longer and an unknown `operation` value | C3 needs it: a moved operation must get a refusal that names its new tool | refusal fixtures, retry counts in the screen |
| C3 | Progressive disclosure: `orient` keeps `operation` (enum `map`, still required, because the gate's refusal tells agents to pass `operation="map"`); communities, cycles, diagram, path and health move to a new tool without the `anthropic/alwaysLoad` hint; `find_referencing_symbols` and `capabilities` lose the hint, and the symbols description keeps the pointer to `find_referencing_symbols`; `no_cache` leaves the MCP surface; `repo_root` is published only when the server has more than one root | one decision: what loads every turn and what is reachable on request | first-turn context, ToolSearch round trips, the screen, then the 296-instance confirmation |
| C6 | Relevance of symbol answers: rank same-rank exact hits by proximity to the session's focus instead of path order, cap substring hits, add a `path` scope to `find`, collapse name-only-ambiguous tiers to a count plus the call-site text with same-directory candidates first. Exact names already rank first (`crates/app/src/find.rs:172`) | all four decide which candidates a symbol answer shows | the surface diff, then the screen |
| C7 | Map hub penalty: rank by the focus-personalized score relative to the global score, so central utility files stop filling the top 10 | | the Loc-Bench retrieval tier first (free), then the screen |
| C8a | Gate hint: when an allowed Grep searches for an identifier, the answer adds that `symbols find` gives the definition | | the hooked screen |
| C8b | Gate re-arm: after 3 native searches for identifier-like patterns, the gate asks for an agentless call again. Literal-text searches stay allowed, because agentless cannot answer them until C10 | follows C8a, so each effect is attributed | the hooked screen; the threshold 3 is the owner's starting value, not a measured one |
| C9 | Gate: refuse an unbounded Read of a large file until a `symbols overview` or a `read` slice of it | | the hooked screen |
| C10 | A text search with structure: literal or regex matches, each reported with its enclosing symbol and stable id, so a quoted error message lands on a function | a new operation; needs its own spec (`/spec`) | spec first |

The C2 draft (`/var/tmp/claude-rust-port/flat/proposed_c2.py`) writes the five always-loaded tools in 7,895 bytes, from 12,868 now (-39%). C3 then takes the always-loaded set to about 4,500 bytes.

Why this order: C2 and C4 are the cheapest, and their size effect is measurable without a model run. C5 comes before C3, because C3's moved operations need its refusals: the server validates arguments before it selects the operation (`crates/mcp/src/dispatch.rs:70`), so an old operation value reaches the validator first. C6 and C7 change what answers contain, so they need the surface diff and the retrieval tier. C8 and C9 change agent behavior through the hook. C10 is a new capability and the largest piece.

## Line numbers in relationship answers

The owner asked whether answers that carry relationships, not code, need line numbers. Measured: line numbers are 1.9% of a `find_referencing_symbols` answer and 3.5% of a `symbols find` answer. The receipt (25 to 38%) and the stable ids (15 to 31%) cost far more. The benchmark's answer is a list of files and line ranges, and the agent reads a call site by its line, so line numbers stay. C4 cuts the receipt and the id display instead.

## Measurement protocol

- **The screen**: `flat60.jsonl` (seed 20261024, 60 of the 296 instances under the walk bound). The old and new builds run in the same window: the forced arm for surface and output changes, the hooked arm for gate changes. Paired bootstrap 95% CI, 20,000 resamples, on the 21 quality metrics, turns, and context and output tokens. The screen detects mechanism (errors, turns, tokens) and large quality changes. It cannot certify that quality did not drop: at n=60 the WCC half-width is about 0.034, wider than the 0.019 noise floor (benchmark-methodology.md, "The agentic tier also cannot certify no-degradation").
- **A placebo per window**: the unchanged build against its last run on the same 60 instances. C1's placebo moved turns -0.5%, context +5.5% and WCC -0.0045, none significant.
- **The confirmation**: the 296-instance set, which clears the about 192 instances a 0.019 WCC margin needs. It runs after C2, C4 and C5 together, and again after C3.
- **Deterministic measures** where they exist: tools/list bytes, first-turn context, answer bytes over the Loc-Bench checkouts. Bytes are not tokens; the first-turn context of a run is the token measure.
- **The release regression gate** in benchmark-methodology.md for every change that ranks, resolves or renders.

## Files each change touches (beyond its own code)

- **C2, C3**: the exact surface fixtures (`crates/app/src/surface/tests.rs`, `crates/app/fixtures/surface/surface.json`, `crates/mcp/src/tests/wire.rs`), the hook test that shares the unlock fixture (`crates/hook/src/search/tests.rs`), the agent guide's operation contract and eager-loading sections (`crates/app/data/agent-guide.md`), and the CHANGELOG.
- **C3**: the gate's unlock list (`crates/app/src/surface.rs` localizing table, `crates/hook`), which names `orient(map|path)`.
- **C4**: the receipt parsers (`crates/mcp/src/tests/calls.rs` masking, `scripts/surface_cmp.py`) and the guide's receipt section.
- **Any change to the orient description**: the token-band tests (`crates/app/src/tokens/tests.rs`), which check that orient publishes "a real BPE count runs 10-35% higher". C2 keeps that sentence.

## Alignment with Anthropic's guidance

- "Simple tool descriptions" and "design interfaces" instead of examples (context engineering for Claude 5, 2026-07-24): C2's enums and short descriptions.
- Progressive disclosure through tool search with deferred loading, and preconditions inside the tools that return an instruction the model can act on (when to fold subagents into the main loop, 2026-10-22): C3 and C5.
- Token-efficient responses and actionable errors (writing effective tools for agents): C4 and C5.
- "Measure, change one thing at a time, and keep what holds up" (2026-10-22): the protocol above.

## Risks

- **0.6.2 deferred every schema and bought nothing**, because the gate made the agent fetch each schema anyway. C3 defers only the tools a localization session rarely calls, and keeps the routing text that tells the agent they exist.
- **Shorter descriptions can change tool choice.** The screen measures it before C2 ships, and the confirmation decides.
- **C8b can cost coverage.** The 300-instance run found that withholding native search costs coverage (forced below hooked). C8b therefore applies only to identifier-like patterns, and literal-text search stays open.
