# Is the tool surface earning its calls?

**Date**: 2026-10-25 | **Status**: analysis, no change proposed | **Method**: paired
tool experiment, source verification, adversarial review by an external model

This document asks one question: does the agentless tool surface deliver enough value
per call to beat the `grep`, `glob` and file-read tools an agent already holds, and if
not, what specifically is wrong with the design. It is a design review, not a benchmark
report, and it does not propose a change.

## Summary

Organized, token-efficient output is not a sufficient reason for an agent to choose
agentless, and the surface's design fault is narrower and more serious than
formatting: **target resolution can fail as a successful-looking negative result, and
in the one Rust symbol sampled here, no cross-crate reference reached a binding tier.**
A tool that reports "no references" for a symbol with 25 gives the agent a reason to
re-check with grep, and that re-check is where a saving of this size would be lost.
Trust is the only asset a structural tool has, and this one is currently breachable
through a target form the server accepts but does not document.

Six of the thirteen folded operations received zero calls across 60 agentic instances.
That is
a real signal and it is not proof of low value: the benchmark that produced it scores
localization against reference reading trajectories, and it never asked for an
import-cycle report, a diagram, or historical rationale.

Four positions from the discussion that produced this document did not survive review
and are recorded overturned rather than quietly dropped. They are in *Where the initial
position was wrong*, because they are the natural reading of this evidence and the next
reader will reach them too.

## Evidence status in this document

Three kinds of claim appear, and they are not equally solid.

**Verified in this tree.** Every `file:line` citation below was opened and read. The
defect reproductions were run against the installed `agentless-mcp` 0.11.0rc1 binary
over this checkout at `9c8b293b`, and the commands are in *How to reproduce*.

**Session-recorded, not archived.** The call counts, byte counts and wall times in the
experiment table are from one interactive session in this workspace. They are not
archived as repository artifacts, so they are not reproducible from this document
alone. Treat them as indicative of shape, not as measurements in the sense
[benchmark-methodology.md](benchmark-methodology.md) uses that word. The defects, by
contrast, reproduce.

**Quoted from the recorded benchmark.** Figures attributed to a dated run come from
[benchmark-methodology.md](benchmark-methodology.md) and carry that document's own
caveats, including the one that marks the August arms' baseline comparisons unverified.

## The experiment

Seven localization questions were run through both tool sets on this repository. The
agentless arm used the CLI form of the same binary the MCP server serves, with a warm
cache. That measures the answer shape and the latency; it does not measure the MCP
schema overhead, which is taken from the recorded 300-instance run.

| Question | bash arm | agentless arm | Won |
| --- | --- | --- | --- |
| Origin of the literal text `no counted edge in or out` | 1 call, 1 ms, 3 hits | cannot express the query | grep |
| Which file sets the config key `erosion_gate`; where the non-UTF-8 refusal lives | 1 call, 1 ms | cannot express the query | grep |
| Where the map packs symbols into the token budget | 3 calls, 72 files mention `budget`, a 12-file list scanned | 1 call, 158 ms, 2,583 B, ranked `app/src/budget.rs` first | agentless |
| Fan-in of `app/src/budget.rs::allocate` | about 4 calls | 1 call, 6 rows, plus the import edge | agentless |
| Does a truncation helper already exist (`shared_callers`) | would need the reference graph rebuilt per task | 1 call: surfaced two duplicate `shorten` functions in two crates plus `kernel::Bounded.cut`, mixed with noise | agentless |
| How symbol A relates to symbol B (`path`) | would need the reference graph rebuilt per task | 1 call, both hops at the weakest tier | agentless, weakly |
| Why `io/src/native.rs` holds `unsafe` | `git log --oneline --no-patch -L :exec_first:crates/io/src/native.rs`, 1 call, 92 B (commit subject) | 4 calls; `history` refuses a file path and demands a symbol stable id | git |

Latency is not the constraint: 7 to 164 ms warm. The cost is context. The
300-instance run reports per-instance mean context of 26,052 tokens for the native
baseline against 71,844 for the hooked arm, +176%. Separately it reports first-turn
medians of 5,518 against 11,010; the difference between those two medians, 5,492, is
derived here rather than measured, and first-turn figures alone do not establish an
overhead on every later turn. The two are different instruments and must not be quoted
as one number.

Two of eight MCP calls in the experiment failed on parameter assembly. On the single
fan-in question where both arms were compared byte for byte, agentless returned 2,800
bytes against grep's 2,625. That is one task, not a token budget, and it does not
establish a general parity in either direction.

## Defects found, with verified root causes

### 1. A target form that resolves to nothing returns a successful empty answer

`find_referencing_symbols` with the target `crates/json/src/float.rs::float_text`
returns:

```
crates/json/src/float.rs::float_text resolves to no symbol; these are the
references to the name float_text, wherever it is defined

no references to crates/json/src/float.rs::float_text outside its own definition
```

This is false. Real call sites outside the definition's own file sit at
`crates/app/src/capabilities.rs:35`, `crates/app/src/file_graph.rs:41`,
`crates/app/src/validate/baseline.rs:194`, `crates/store/src/value.rs:77` and
`crates/agentless/src/server/tests.rs:107`; three more, at
`crates/json/src/write.rs:97,173,319`, are cross-file but inside the `json` crate. The
prefixed stable id `rs:crates/json/src/float.rs::float_text` returns 25 references, so
the index holds the data. The `reference_target` description at
`crates/prompts/data/parameter_descriptions.json` promises a "Stable id, qualified
name, or symbol name", and the unprefixed `path::name` spelling is not among the forms
it documents, so this is an accepted-but-undocumented form returning nothing rather than
a documented one being broken. What makes it a defect rather than a gap is that the
server accepts the form, announces a name fallback, and then returns nothing, so the
message and the result contradict each other.

The root cause is narrower than the interface complaint it resembles. A failed stable
id parse falls into the dotted-name lookup, whose final-component parser does not
understand `path::name`; reference collection then iterates the empty definition set.
See `crates/graph/src/lookup.rs:19` (`resolve_definitions`),
`crates/graph/src/lookup.rs:88` (`last_name`) and
`crates/app/src/refs.rs:211` (`spellings`). It is a parser gap plus a missing refusal.

### 2. No cross-crate reference reached a binding tier in the sampled symbol

`capabilities` reports `scope analysis (a parameter or local is told from a caller):
python`. Measured on `float_text`: of its 25 references, every cross-crate Rust call
site is in `name-only-ambiguous`. The only `resolved-via-import` row is inside the
`json` crate's own `lib.rs`, and the `same-file` rows carry `locals not checked`.

One symbol does not establish a repository-wide proportion, and this claim should not
be read as one. What it establishes is that the strongest tier the tool offers for a
cross-crate Rust call was not reached in the case examined.

Two limitations are conflated in the phrase "Rust resolution is weak" and should be
tracked separately: missing local-scope analysis, which is what `capabilities` reports,
and missing cross-crate import resolution for Rust, which is what this case
demonstrates. Neither implies that Rust symbols have no useful structural evidence; the
fan-in rows on `allocate` were real, subject to defect 3.

### 3. `unique` is not a binding tier, and the rendering implies it is close to one

`Tier::binds` at `crates/graph/src/resolver.rs:52` is
`matches!(self, Tier::SameFile | Tier::Imported)`. `unique` does not bind; it is itself
one of the candidate tiers. The supported objection is about the headline, not the
ordering: `groups_text` at `crates/render/src/refs.rs:74` prints the total across every
tier, and groups are keyed by `(path, Tier)` at `crates/app/src/refs/sites.rs:15`, so
rows sort by file first and tier second rather than by tier globally
(`crates/app/src/refs/sites.rs:40`). A reader therefore sees one count that mixes
binding evidence with candidates and must read each block's tier label to separate
them. That is how the initial reading of this experiment mistook six `unique` rows, two
of which were module-level rows rather than callers, for six bound callers.

### 4. `path` is undirected

`crates/graph/src/path.rs:113` builds `adjacency: undirected(graph, policy)`. The tool
finds a relationship chain, so `A <- C -> B` satisfies it. That is not reachability
from A to B and it is not transitive closure. Presenting it as a change-propagation or
call-flow tool is wrong, and the name invites that reading.

### 5. False cross-language edges into the project's own fixtures

`explain` on `rs:crates/app/src/budget.rs::allocate` lists a fan-out reference to
`Inventory.remove` in
`py:tests/characterization/fixtures/repo_py/inventory.py`. The call needs the prefixed
stable id: the unprefixed `crates/app/src/budget.rs::allocate` meets the same lookup
defect as defect 1 and returns no symbol (`crates/app/src/explain.rs:119`). A Rust
production function
is linked to a Python test fixture by spelling alone. The check-time exclusion and the
graph are separate scopes serving separate purposes, so this is not a scope
inconsistency; it is a candidate-tier edge that should not be emitted across languages
at all.

### 6. Folded operations publish a schema their runtime refuses

`crates/app/src/surface.rs:456` (`input_schema`) publishes the union of a tool's
operation parameters, while `crates/mcp/src/dispatch.rs:72` validates the call against
the selected operation, and `checked_operation` at `crates/mcp/src/operation.rs:16`,
via `refused_parameters` at `crates/mcp/src/operation.rs:40`, rejects parameters that
operation does not take. Two of eight MCP calls in the experiment failed this way. The
validation is correct and runs before the call is accepted. The defect is not that the
schema cannot be satisfied, because a valid operation call does satisfy it; it is that
the published schema also admits parameter combinations the selected operation will
reject, so the caller learns the real contract only by being refused.

## Where the initial position was wrong

**"Exactly four capabilities are irreplaceable by bash."** Too strong, and the
criterion is wrong. Bash can execute graph algorithms and semantic analyzers; what it
cannot avoid is reconstructing their work inside every task. The defensible line is
*work the server reliably eliminates*, not exclusivity. The error runs the other way
too: extracting a complete function body or a file's declarations needs syntax
boundaries that grep cannot supply reliably across languages, so symbol-scoped reading
is not "grep with better typography".

**"An enrichment hook can be built on the advisory channel that already exists."** It
cannot, and the gap is more specific than review first suggested. `advisory_reply` at
`crates/hook/src/verdict.rs:151` hardcodes `hook_event_name: "PreToolUse"`, so the
advisory path runs before the tool and annotates an edit that has not landed. A
`PostToolUse` handler does exist at `crates/hook/src/search/mark.rs:35` (`gate_mark`),
but it parses the payload to set the session marker and write a log line; it reads the
event fields, not the tool's output, so it neither inspects nor alters the result
(`crates/hook/src/search/mark.rs:51`). What is missing is a handler that receives a
search result and returns an enriched one, which is a different integration point from
either existing handler.

**"Never return an empty success."** Wrong as stated. A resolved symbol can
legitimately have zero observed references, and suppressing that is worse than
qualifying it. The right contract separates `binding.status` (`bound`, `ambiguous`,
`not_found`) from `coverage.status` (`complete`, `partial`, `not_checked`), so a
completed zero is reportable and a failed bind is not mistaken for one.

**"The substitution join is a query over data already collected."** Overstated.
`crates/hook/src/search/check.rs:125` (`entry`) logs `ts`, `session_id`, `tool`,
`pattern`, `event` and `reason`, with the key names at
`crates/hook/src/search/log.rs:18`. It does not log the search scope, the flags, the
returned match locations or any enrichment outcome. Joining `pattern` against declared
symbols gives a bounded estimate of missed opportunity, not a substitution rate. The
missing fields are cheap to add and are the ones the argument needs.

Two further narrowing corrections from the same review: the lookup-then-handle
criticism applies to symbol expansion, not to reading, because `read(path, lines)`
takes a path and a line range directly with no preliminary lookup
(`crates/app/src/surface.rs:395`); and the exposure figures cannot be attributed to
model drift, because nothing in them holds the task set, server, interface and model
constant while varying prompt adherence.

## What the recorded evidence does and does not support

The exposure numbers are the strongest signal in the corpus. On the 2026-09-27 hooked
arm, the methodology records 359 calls over 60 instances while the list it enumerates
sums to 358; this document does not resolve the one-call discrepancy. The distribution:
`symbols(expand)` 109, `read` 93, `orient(map)` 61, `symbols(find)` 61,
`symbols(overview)` 19, `find_referencing_symbols` 10, `symbols(explain)` 5, and
`cycles`, `communities`, `path`, `health`, `diagram` and `history` zero. That is 13
reported categories, seven used and six not, against the 13 operations folded behind
`orient`, `symbols` and `read` plus the standalone tools; the enumeration is not a
complete inventory of every operation the server publishes, and a zero here means
unreported in that run rather than proven unused.

**Task demand explains much of it, and zero calls is not zero value.** A localization
benchmark needs declarations and bodies. It rarely needs an import-cycle report, an
architecture diagram, or historical rationale. The methodology says each instrument
measures only what it can observe, and it scores localization against reference reading
trajectories, not refactoring, architectural investigation or repair. Deleting the six
unreached operations on this evidence would remove a discovery path on the strength of a
benchmark that never asked for one.

**"Model drift away from prompts" is not established.** The claim may be true. Nothing
here measures it, and it is currently doing argumentative work it has not earned.

**The interface-friction evidence is the part that is solid.** Operations share a
schema and accept different parameters; symbol expansion requires a lookup-then-handle
sequence; a failed bind can appear as a successful negative; the output mixes binding
evidence with spelling candidates. All four are observable in this tree, named above,
and all four are fixable without a model.

The 2026-10-24 300-instance run **detected no improvement** in coverage from the
agentless arms against the native baseline: WCC, recall and F1 intervals all contain
zero, at +176% context and +1.2 turns. Containing zero is not equivalence and not
proof of no benefit; at n=300 it is an absence of evidence, and the methodology warns
against reading it either way. The same run found the forced arm, which withholds
native search, worse than the hooked arm on WCC (-0.0146, interval -0.0249 to -0.0048),
and that comparison does support keeping native search available. It does not
distinguish the current deny-until-one-call gate from no gate at all, because no arm
tested that. The 0.6.1 result pointing the other way is weaker than it looks: the
methodology marks the August *baseline comparisons* unverified for session isolation
(`docs/analysis/benchmark-methodology.md:124`). That caveat attaches to the comparisons,
not explicitly to the "gate denied a call in 7 of 60 instances" count
(`docs/analysis/benchmark-methodology.md:368`), which is an incidence within the hooked
arm rather than a baseline contrast. It is still an August figure under a session model
that was later found to leak operator configuration into every child session, so it is
cited here as a rough order and not as the gate's measured cost.

## The pivot, re-sequenced

Ordered by what the evidence supports, not by size of change.

**1. Repair target resolution and make refusals honest, before anything else.** Accept
bare names, `path::name`, prefixed stable ids, `file.rs:116` and bare paths; preserve
each form's scope, so an explicit file-qualified target never silently broadens; fix
the `path::name` component parser; and return `binding.status` and `coverage.status`
so `not_found`, `ambiguous`, `partial` and a completed zero are four different answers.
Delete the message that promises a fallback it does not perform. This is a parser fix
plus a contract, and it is the defect that can turn a saving into a re-verification.

**2. Promote binding evidence and demote candidates in the rendering.** `unique` does
not bind. Either count only binding tiers in the headline rows and put candidates under
a separate explicitly uncounted heading, or stop counting them. The tier vocabulary is
honest and the layout is not.

**3. Rename `path` and add direction.** `find_relationship_path` is what it computes
today. Add a direction parameter before anything treats it as change propagation.

**4. Make the per-operation contract visible to the caller.** Validation already runs
before acceptance at `crates/mcp/src/dispatch.rs:71`; the gap is that the published
schema promises parameters the selected operation rejects. Publish per-operation
schemas, or encode the accepted parameter set per operation in the schema the caller
sees. Rejection after commitment is a round trip the caller cannot plan around.

**5. Do not emit cross-language spelling candidates.** Keep real test-to-production
edges; drop the name-only links between a Rust symbol and a same-spelled Python fixture
method, which carry no binding evidence in either direction.

**6. Removing the deny gate is a judgment call and should be recorded as one.** The
case for it is that withholding native search measurably costs coverage, so a gate
whose purpose is to redirect search is aiming at the wrong thing. There is no measured
comparison of gated against ungated, and the 7-of-60 incidence figure comes from an
unverified era and should not be cited as the gate's cost.

**7. Then run the comparison that has never been run.** Ungated, gated, and enriched,
with identical schema exposure across the three and a native-only control. Measure
successful localization, total context, repeated searches over the same concept, and
malformed call rate. Enrichment is a hypothesis, not a plan: it needs a handler that
sees results, it cannot recover callers that use an alias the agent never searched for,
appended context still costs context, and moving ambiguous edges into grep output can
make them read as more authoritative than they are.

**8. Instrument the opportunity, then the substitution.** Add search scope, flags,
returned match locations and enrichment outcome to the gate log. Then report what
fraction of `Grep` and `Glob` calls had a pattern that is a declared symbol name, and
of those how many the server could have answered at a binding tier. Today that is a
bounded estimate at best, and the difference between an estimate and a measurement is
the four fields above.

## What this document cannot conclude

It cannot say agentless is not worth building. The instrument that measures adoption
scores localization against what a reference trajectory read, and the six unreached
operations were never part of that question.

It cannot attribute the exposure pattern to model drift, because nothing varied prompt
adherence against a fixed interface.

It cannot rank agentless against grep under any other interface. The isolated
300-instance run compares the current surface against a native baseline that runs no
MCP server at all, and the older arms ran earlier builds, so those runs are evidence
about the surface they exercised and silent about the replacement proposed here. That
silence is about the proposals in this document, not a defect in the runs themselves.

The claim it can defend is narrower and actionable: **the first-order problem is that
the tool can be wrong in a way that looks right, and it is cheap to mis-operate in ways
that are visible in the source.** Fix the binding, fix the refusal, make the operation
contract visible, then measure. A cut to the surface is not supported by evidence that
never asked whether the cut surface was needed.

## How to reproduce

Defect 1 reproduces through the CLI, which forwards the target string to the same
application function the MCP tool calls (`crates/cli/src/read/refs.rs:9`):

```bash
agentless-mcp index --repo .                                # warm the cache once
agentless-mcp refs "crates/json/src/float.rs::float_text" --repo .   # returns zero
agentless-mcp refs "rs:crates/json/src/float.rs::float_text" --repo . # returns 25
```

The other claims that were checked in source:

```bash
grep -n "fn binds" -A3 crates/graph/src/resolver.rs         # unique does not bind
grep -n "undirected(graph" crates/graph/src/path.rs         # undirected adjacency
grep -n "PreToolUse" crates/hook/src/verdict.rs             # advisory is pre-tool
grep -n "pub fn gate_mark" crates/hook/src/search/mark.rs   # post-tool marker, no result
git log --oneline --no-patch -L :exec_first:crates/io/src/native.rs   # 92 B
```

The experiment table's call counts and byte counts are session-recorded and not
archived, so they do not reproduce from this document. One qualification from review:
during the review pass a connected MCP server accepted a flat `lines` pair while this
checkout declares nested pairs (`crates/app/src/surface.rs:147`). Pin the executable
build, not only the analyzed repository, before comparing behavior across builds. That
is the same error class as incident 3 in
[benchmark-methodology.md](benchmark-methodology.md).

## Provenance

The experiment, the initial position and the first defect list are this session's. The
position was then reviewed adversarially by an external model, `codex-cli` 0.162.0 at
`gpt-6.1-sol`, read-only, with access to this repository, and instructed to argue
against it. That review overturned the four-verb claim, the existing-advisory-channel
claim, the never-return-empty rule and the already-collected-data claim, and located
the `path::name` parser root cause and the undirected `path` adjacency. A second pass
over this document objected to seventeen points, of which the substantive ones were the
conflated context-token figures, an overstated reproduction claim, an unarchived
evidence trail, and several claims stated past their evidence. Each objection was
checked against the source before being applied, and the applied corrections are what
this document now says. Positions the review raised and this document does not accept
are defended in *What the recorded evidence does and does not support*.
