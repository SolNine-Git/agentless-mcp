# Specification: callee_overlap, a re-implementation check for `agentless lint`

Ticket SPEC-CO. Date 2026-11-03. Base: `feat/agent-surface-efficiency` at aa6d127. Status: design, no code.
Work record: /var/tmp/claude-rust-port/spec-co/status.md (W). Every count below names its source in W.

## 1. Problem statement

`clone_delta` and `near_duplicates` match exact hashes. They find Type-1 and consistent Type-2 copies only
(research report K7). An agent that writes a new function with different code for an existing job passes both
checks. The owner wants a deterministic check for this case (confirmed 2026-11-02).

The proposed rule comes from RepoReuse (arXiv 2609.35357, sec. 3.3.2). Flag an existing function g when:
1. g has 3 or more counted callees.
2. The added function f does not call g.
3. f calls 80% or more of the counted callees of g.

RepoReuse applies the rule only to the known targets of a reference solution, in Python, with name matching.
Nobody has measured its precision over a whole repository. This specification decides the design, and it
makes the gate depend on a measured false-positive rate.

## 2. Research findings

| # | Finding | Source |
|---|---|---|
| F1 | The rule counts "non-builtin" callees, finds calls by "AST-based value-reference name matching", and checks only the targets of the reference solution. | RepoReuse sec. 3.3.2, read at arxiv.org/html/2609.35357 |
| F2 | Oreo uses called methods and accessed fields as "Action tokens". It builds an inverted index on them. The overlap only selects candidates. A trained model then decides. | Oreo sec. 2.3-2.5, read at ar5iv 1806.05837 |
| F3 | Copy-paste detectors reach 93-100% recall on Type-1 to strong Type-3, and 0-1% on weak Type-3 and Type-4. Token tools have 97.8-99% precision. Oreo has 89.5%. | Oreo Table 3, via the report sec. 4.1 |
| F4 | Duplication includes code that is "similar but not the same", and methods that "do the same thing with a different algorithm". | Fowler, *Refactoring*, "Duplicated Code" |
| F5 | Functions that "share common code at the start and end" are a symptom of duplicate code. | Thomas and Hunt, *The Pragmatic Programmer*, "Avoid similar functions" |
| F6 | A short list of existing functions raised self reuse from 30.0% to 67.8%. | RepoReuse sec. 4.4 (report K6) |
| F7 | Oreo and RepoReuse are not in the docs corpus. A papers search found neither. | `search_all_docs`, 2026-11-03 |

Code findings (W/clone at aa6d127, read 2026-11-02 and 2026-11-03):

- C1. `PairCheck` holds three checks. `name` (registry.rs:75-81), `keeps` (pairs.rs:82-88) and `run_check`
  (pairs.rs:90-113) are parallel `match` ladders. `PairSelection` stores `chosen: [bool; 3]` (registry.rs:101-103).
- C2. `match_run` (matching.rs:63-91) pairs units by body within a file, then by body across files (a move),
  then by name, then by rank. A unit with no partner is `Match::New`. `body_key` leaves out the unit's own name,
  so a rename without other change pairs by body (matching.rs:123-124).
- C3. `Tier::binds` (resolver.rs:52-55) is true for `same_file` and `imported` only.
- C4. `SymbolGraph::build` (symbols.rs:239-254) resolves every reference and checks a `Deadline`.
  `with_resolved` (app/src/symbol_graph.rs:10-20) builds it. `explain`, `path` and `health` use it.
- C5. `checks` and `graph` are both layer 3 (crates/layers.toml). `checks` cannot import `graph`.
- C6. Rust references get `Role::Reference` for every identifier (identifier_refs.rs:23-33). No role separates a
  call from a field, a parameter or a local.
- C7. `Symbol` (facts/src/values.rs:18-39) has no trait-impl flag and no test flag. The Rust walker reads
  `impl_item` but drops its trait (rust_items.rs:91-100).
- C8. Python builtins have one home: `builtins` in syntax/languages/python.toml:69, read by
  `Extraction::is_builtin` (syntax/src/extraction.rs:134-136). Rust has no such list.
- C9. `cargo xtask erosion` runs `lint --since` with `--fail-on warning,advisory,not_checked`
  (xtask/src/erosion.rs:10-11, 307-318). A check in its `CHECKS` list gates at any severity.
- C10. By default no finding fails `lint` (agent-guide.md:1458-1463). `clone_delta` is opt-in with `--only`,
  and it reports `Severity::Advisory` (checks/src/clone.rs:109). The hook gates `GatedCheck::ALL`
  (io/src/config.rs:102-120).
- C11. The clone index stops at `WALK_BUDGET`, 10 s (app/src/env.rs:15; checks/src/index.rs:52-58).

## 3. Measurements before design (all derived unless marked measured)

Method: W/scripts/callee_sizes.py joins every in-scope function with every other one over a GF dump (one row per
resolved site) and the tag-cache spans. This is a whole-repository self-join, not a lint run. W/scripts/
classify_pairs.py then removes test functions, Rust trait-impl methods and Python dunder methods by text
heuristics. Counts are ordered pairs. "ship" is the GF1 build; "off" is the base.

| Snapshot | Functions in scope | Eligible g, binding (off -> ship) | Kept pairs, binding | Kept, binding+unique | Kept, b+u, family limit 5 |
|---|---|---|---|---|---|
| this repo c2daf41 | 4,425 | 433 -> 829 | 41 | 151 | 151 |
| ripgrep-2576 | 1,791 | 121 -> 204 | 28 | 5,398 | 131 |
| tokio-6752 | 4,085 | 22 -> 99 | 55 | 822 | 369 |
| matplotlib-13989 (off) | 9,726 | 417 | 238 (28 with limit 5) | 238 | 28 |

- Filters matter. Before them, test functions are 208 of 251 binding pairs here. Trait-impl methods are 263 of
  1,091 b+u pairs in tokio.
- Sibling families are the main false-positive class after the filters. ripgrep `crates/core/app.rs` flag
  builders give 5,279 of 5,398 b+u pairs. matplotlib `pyplot.py` wrappers give 211 of 238. tokio `fs` wrappers
  around one helper give the largest group there. The family limit (section 4, D4) removes these floods. It
  removes no pair in this repository.
- Work per function is small. Under binding, f meets at most 47 (here), 51, 8 and 66 candidate g. The inverted
  index holds 3,541, 1,078, 418 and 1,978 postings.
- Measured on 2026-11-03 (binary 18ffbb9, warm cache under W, two runs, `/usr/bin/time`): one `explain` call
  builds the whole symbol graph in 0.21-0.22 s and 142 MB here. ripgrep: 0.03 s. tokio: 0.13 s. matplotlib:
  0.23-0.24 s.
- Pilot labels, read by this agent and not by a human (W/data/pilot*.jsonl), seeded samples of this repo:
  b+u, 1 duplicate in 11 distinct pairs. Field, parameter and local names give 4 of the false positives (C6).
  Binding, 2 partial duplicates in 6 pairs (`diagram`/`html` and `slice`/`opened` share one preamble). The
  other 4 are a CLI builder family and two sibling renderers. The samples are small. They do not set a number.
  They show the causes.

## 4. Design decisions

**D1. Where the check lives.**
- Name: `callee_overlap`. It is a pair check. It runs only in the `--since REV` form, and only when `--only`
  names it. It is not in the default selection (registry.rs:108-112), because it reads the whole tree.
- `--path` with `--only callee_overlap` is a usage error (exit 2) with its own refusal row. The hook form reads
  only the tag cache, and a stale cache gives stale callee sets.
- The pure core goes in a new module `crates/checks/src/callees.rs`. It takes typed callee sets and returns
  findings. The shell goes in `crates/app/src/callees.rs`. It scans the tree, builds the graph with
  `with_resolved`, and makes the callee sets. This keeps `checks` free of `graph` (C5).
- Severity: `Severity::Advisory`. The input value in `capabilities` is `pairs+graph`.
- Phase 1 does not add the check to `xtask/src/erosion.rs` `CHECKS` (C9) or to `GatedCheck` (C10). Thus it
  cannot gate before the measurement.
- Finding text, a new `messages.json` row `callee_overlap_found`:
  "`{name}` calls {shared} of the {total} functions that `{other}` calls, but it does not call `{other}`. If
  the two do one job, call `{other}` and remove the copy. If they only use the same helpers, ignore this row."
  Evidence row `callee_overlap_found_evidence`: "`{other}` at {site}; shared: {callees}". The location is f;
  the evidence names g by `path:line`, as `clone_delta` names its twin.
- JSON `detail`: `unit`, `match`, `other`, `other_path`, `other_line`, `family`, `shared` (each callee id
  with the tier of the f edge and of the g edge), and `other_callees` (each callee of g with its tier).
  With these fields, a reader can score another tier policy from one run.

**D2. Counted callees.** A callee is a function or method definition of the repository that an edge from the
function reaches. Count an edge only when `Tier::binds()` is true (C3). Do not count `unique` or
`name-only-ambiguous` edges.
- Reason: the pilot shows that `unique` edges in Rust are mostly field, parameter and local names (C6).
  `Tier::binds` is the existing one home of the rule.
- Cost to recall: a method is not a top-level definition, so a Rust call on a receiver (`x.m()`) never binds
  at `same_file`. It resolves as `unique` or `name-only-ambiguous` (resolver.rs:170-195), so it never counts.
  Eligible g here: 829 binding against 1,706 b+u.
- Python has role analysis (identifier_refs.rs:12). Only `Reference` and `ModuleAttribute` resolve
  (values.rs:224-226). Thus Python parameters and locals give no edge, and the false-edge class of C6 is a Rust
  problem. A Python attribute call (`self.m()`) also gives no edge at any tier. A `module.name` call binds
  through its import.
- Cost to precision of the other choice: b+u kept 151 pairs here against 41, and the pilot read 1 in 11.
- Phase 1e (conditional, D7) adds a call-position flag to references. Then `unique` edges in call position
  can count. Phase 1e runs only if planted recall is low.
- GF1 dependency: yes, the Rust measurement must wait for the GF1 build. Before GF1, Rust cross-file edges
  bind at 679 of 139,296 sites here (gf/design.md), so the check sees almost only same-file pairs. Eligible g
  doubles with GF1 (433 to 829). The Python measurement does not wait. GF1 changes only Rust binding and
  cross-language candidates, and binding tiers do not use candidates.

**D3. Added function.**
- The change set is the `TextPair` list of `lint --since` (lint.rs:130-134): old text from git, new text from
  the working tree.
- Use `match_run` (C2) on the metric units of each pair. Check every unit with `Match::New`.
- A moved or renamed function without other change pairs by body, so it is not new. A function moved to
  another file and edited in the same change reads as new. Its old copy is gone from the tree after the
  change, so it cannot match itself.
- A changed function (`Match::Name` or `Match::Collision`, body changed) is checked against g only when its old
  body spelled fewer than 80% of the names of g's callees. The old side has no symbol graph, so this delta
  uses names. Reason: a row must be about the change, not about old code near the edit.
- "f does not call g" means that f does not spell the name of g at all, at any tier. This is conservative.
- Join units to graph symbols by path, qualified name and start line.
- g ranges over every in-scope function of the tree after the change, except f. When f and g are both new,
  report the pair once.

**D4. Filters, each as data with one home.**

| Filter | Home | Rule |
|---|---|---|
| Path scope | `in_metric_scope` (app/src/scope.rs:32-36) | f and g are outside test and fixture paths, as for `clone_delta` |
| Languages | a `callee_stoplist` key in syntax/languages/rust.toml and python.toml | the check runs on a file only if its language table has the key |
| Stoplist | the same key | `new`, `clone`, `into`, `push`, `len`, `unwrap` in both tables. An edge to a definition with a listed name never counts |
| Protocol methods | new fact `Symbol.protocol` | Rust: a method in `impl Trait for T`. Python: a dunder method. Neither f nor g is a protocol method |
| Test functions | new fact `Symbol.test` | Rust: `#[test]` or `#[tokio::test]`-like attribute, or inside a `#[cfg(test)]` module. Neither f nor g is a test |
| Builtins and std | the graph itself | edges exist only to repository definitions, so `std`, `core`, `alloc` and Python builtins add no callee |
| Thresholds | constants in checks/src/callees.rs | `MIN_CALLEES = 3`, share as `5 * shared >= 4 * total`, `FAMILY_LIMIT = 5`, `MAX_ROWS_PER_UNIT = 3` |

- The builtins row needs no new list. Python builtins already have one home: python_scopes.rs:105 gives a
  name from `python.toml` `builtins` its builtin role, and that role does not resolve (values.rs:224-226).
  Rust needs a list only if phase 1e admits `unique` edges. Phase 1e decides it.
- Family limit: skip g when 5 or more existing functions besides f already meet the rule for g. Such a set is
  a convention, not a copy. Section 3 measures its effect. Use the trait and test facts, not the module name
  `tests`: a guard keys on the invariant, not a proxy.
- Order: report at most 3 rows per f. Rank by shared count, then by the size of g's set (larger first), then
  by path and line.

**D5. Cost and bound.**
- Work: one scan and one graph build of the tree, as `explain` does (0.03-0.24 s measured, section 3). One
  pass over the edges makes the callee sets. One inverted index maps each callee to the eligible g. For each f,
  walk the postings of its callees. Count shared callees per g. Compute the family count for each candidate g
  with the same index. The work per f is the sum of the postings of its callees.
- Bound: the scan and the graph build take a `kernel::Deadline` of `WALK_BUDGET` (10 s, C11).
- Deadline behavior: all or nothing. If the build stops, give one `not_checked` row
  (`callee_overlap_stopped`) and no finding. A partial edge set makes g's set smaller and can make a false row.
- When `clone_delta` also runs, the run reads the tree twice. Phase 1 accepts this and measures the wall time
  of `--since HEAD~5` and `--since HEAD~50` here, as the guide does for `clone_delta`.

**D6 and D7** are the measurement plan (section 5) and the phases (section 6).

## 5. Measurement plan

Write the prediction down first. Prediction: under D2, whole-repository precision falls between 20% and 50%.
Bar B fails. Bar A is uncertain. The likely result is "stays advisory".

| Source | Items | Gives |
|---|---|---|
| M1 planted pairs | per language (Rust, Python): 10 positives (3 renamed copies, 3 edited copies, 4 re-implementations with the same helpers and other code) and 8 negatives (sibling pipeline `load_config`/`load_manifest`, builder-family member, protocol method, test function, moved function, renamed function, f that calls g, g with 2 callees) | recall per kind; 0 rows on negatives |
| M2 history | miner over the 383 non-merge commits. The pool is 95 commits that delete 1-5 functions with no same-name addition (W/data/history-deletions.txt, regex count, upper bound). Keep a commit only when every call site of the deleted d now calls an existing g | recall on known duplicates |
| M3 merged PRs | all merged PRs in W/data/merges.txt (#1-#63) except #62, which adds the whole Rust tree. Run #62 apart and report it apart | every finding read; the false-positive rate on real changes |
| M4 whole-repo sample | 60 seeded pairs from the self-join of the 4 snapshots in section 3, under the phase 1 policy | enough findings for the rate (M3 volume is low: 0.009 kept pairs per function here) |

Procedure:
- M1: plant same-file and cross-file positives. Cross-file Rust positives are expected misses before GF1. The
  golden records them, and the GF1 merge changes the golden by a predicted diff.
- M2: for each kept (d, g), find the commit A that added d. Run `lint --since A^ --only callee_overlap` in a
  detached worktree at A. A hit names g. Expect 5-30 confirmed pairs (not measured). If fewer than 10 remain,
  report recall as "not measured" with the count.
- M3: for a merge M, make a detached worktree at `M^2`. Run `lint --since "$(git merge-base M^1 M^2)" --only
  callee_overlap --json`. Read every finding. Python-era PRs run on Python code.
- Never point a run at the shared working tree. Use detached worktrees with an absolute `XDG_CACHE_HOME` (AGENTS.md).

Readers and labels:
- Two agent readers label each finding of M3 and M4 blind. They see the two bodies and the shared callees. They
  do not see the tier, the score or the order of f and g. This is the method of report section 9.
- Labels: `duplicate` (one job in two homes: f can call g, or both can call one extracted function of 3 or more
  statements, F4 and F5); `sibling` (same helpers, other job); `false_edge` (a shared callee is not a call).
- The owner decides each disagreement. The owner also reads 20 seeded findings. If agent-owner kappa is below
  0.6, the owner labels every finding.
- False-positive rate = (`sibling` + `false_edge`) / findings read. Report the Wilson 95% interval.

Stop rules:
- S1: after 20 findings read, if the lower Wilson bound of the false-positive rate is above 40%, stop. Bar A
  cannot pass. Keep the check opt-in and report to the owner.
- S2: stop reading at 60 findings from M3 and M4 together.
- S3: if the measurement uses twice its estimate, stop and report (the review-loop budget rule).

Bars:
- Bar A, hook advisory: false-positive rate 40% or less over 40 or more findings. Then the hook shows the row
  and allows the edit. Reason: F6 says a pointer to an existing function raises reuse. At this rate, 3 of 5 rows
  name a real duplicate. This number is a judgement (Q4).
- Bar B, block (erosion `CHECKS` and hook `block`): false-positive rate 10% or less, upper Wilson bound 20% or
  less, over 60 or more findings, and M1 re-implementation recall 50% or more per language. Reason: the best
  Type-3 tool in F3 has 89.5% precision. The exact-hash checks that block today are near 98%. Each false block
  costs an agent one turn.

## 6. Phases

**Phase 0: refactor, no change in behavior.** Replace the `PairCheck` ladders (C1) with one table: one row per
check with its name, input, path rule, forms and run function. `PairSelection` keys on the table. Proof: every
suite and golden passes unchanged. This step obeys the parallel-ladder rule of the owner's CLAUDE.md.

**Phase 1: advisory check and its measurement.**
1. Facts: add `Symbol.protocol` and `Symbol.test` (D4). Bump the facts version. Every tag cache re-indexes once.
2. Language tables: add `callee_stoplist` to rust.toml and python.toml. Run `cargo xtask generate`.
3. Core (checks) and shell (app), D1 to D5. Add the table row `callee_overlap` with input `pairs+graph`.
4. Prompts rows: `callee_overlap_found`, `callee_overlap_found_evidence`, `callee_overlap_stopped`, and the
   `--path` refusal. Each row gets a `render.json` fixture with sample values.
5. Fixtures M1 and their goldens. Predict each golden diff before you keep it.
6. Guide, capabilities and CHANGELOG (section 7).
7. Run M1 to M4. Write the record to docs/analysis/ and the result to CHANGELOG.md.
8. Phase 1e, only if M1 Rust re-implementation recall is below 50%: add a call-position flag to Rust
   references. Count `unique` Rust edges in call position. Add a Rust builtin list if the data needs it.
   Python method calls need attribute resolution, which is out of scope here. If Python recall is below 50%,
   report it to the owner.
   Run M1, M3 and M4 again for both policies from the `detail` fields.

**Phase 2: gate, only on a measured bar.** Bar A: add `CalleeOverlap` to `GatedCheck` with default mode
`advisory`. Bar B: also add it to erosion `CHECKS`, to `.agentless-mcp.json` `erosion_gate` as `block`, and a
`scripts/gate-selftest.sh` row that proves the gate fails on a planted copy. A change to xtask/ or a policy file
needs the `Policy-Change:` trailer and the gate self-test (AGENTS.md).

## 7. Files to change (phase 1)

| File | Change |
|---|---|
| crates/app/src/registry.rs, crates/app/src/pairs.rs | phase 0 table; then the `callee_overlap` row |
| crates/app/src/registry/tests.rs, crates/app/src/pairs/tests.rs | table tests; selection, refusal and path-rule tests |
| crates/checks/src/callees.rs (new), crates/checks/src/lib.rs, crates/checks/src/tests.rs | the core and its unit tests: thresholds, family limit, row order, the delta rule, one row for a new-new pair |
| crates/app/src/callees.rs (new), crates/app/src/lint.rs, crates/app/src/lint/tests.rs | the shell: scan, `with_resolved`, callee sets, unit join, deadline row |
| CLI lint wiring (crates/cli/src/commands/) | give lint the scan inputs; `--path` refusal |
| crates/facts/src/values.rs, extract/rust_items.rs, extract/python_definitions.rs | `protocol` and `test` facts |
| crates/facts/fixtures/extract_rust.json, extract_python.json; the store facts-version test | new fields; version bump (predict the diff) |
| crates/syntax/languages/rust.toml, python.toml; crates/syntax/src/generated/ | `callee_stoplist`; regenerated tables |
| crates/prompts/data/messages.json, crates/prompts/fixtures/render.json, template counts in crates/prompts/src/tests.rs and xtask/src/generate.rs | the four rows |
| crates/app/src/capabilities.rs and its fixture | the new check and `pairs+graph` |
| crates/app/data/agent-guide.md (lint section near lines 1350-1450) | `callee_overlap`: rule, filters, `detail`, `--since` only, advisory, measured cost |
| tests/characterization/fixtures/callee_overlap_rust, callee_overlap_py (new); goldens/lint/ | M1 |
| CHANGELOG.md, docs/analysis/<date>_callee-overlap-measurement.md | the entry with each measured figure; the record |

Not in phase 1: xtask/src/erosion.rs, crates/io/src/config.rs `GatedCheck`, .agentless-mcp.json, scripts/gate-selftest.sh.

Verification: the AGENTS.md command list in its order; a golden diff predicted for each changed golden; the
retrieval tier is not needed, because nothing ranks, resolves or renders differently outside `lint`. The facts
change can change `skeleton`, `find` or `refs` JSON if the new fields serialize. Run the surface diff on the
Loc-Bench checkouts if any read golden changes.

## 8. Risks

- Precision: sibling pipelines and small builder families stay under the family limit (pilot B2-B5). The check
  can fail bar B. That result is valid, and the check then stays opt-in.
- Recall: D2 never counts method calls. Phase 1e is the measured remedy.
- Rust before GF1: the check sees same-file pairs only. Measure Rust only on the GF1 build.
- Overlap with `clone_delta`: a shared preamble can also be a Type-2 block (pilot B0, B1). Count the overlap in
  M3 and M4. A row that `clone_delta` also gives adds no new information.
- Bias: this repository already removed many duplicates (M2), so M3 can show a low base rate of real findings.
  M4 adds three outside snapshots.
- Facts change: a version bump re-indexes every cache once. Any test that pins facts changes deliberately.
- Stoplist growth: each new name must go in the language table with its measured effect, never in code.

## 9. Open questions for the owner

1. Tier policy. Recommended: binding tiers only in phase 1. Add the call-position arm (phase 1e) only on low
   planted recall.
2. Changed functions. Recommended: check them with the name-level delta of D3. The other choice is new
   functions only, which misses an edit that rewrites a function into a copy.
3. Family limit. Recommended: 5, as a constant. Section 3 shows that it removes the floods and nothing here.
4. Bars. Recommended: bar A at 40% false positives for the advisory hook; bar B at 10% (upper bound 20%,
   60 findings) for block. Confirm both numbers.
5. Readers. Recommended: two blind agent readers, the owner on disagreements and on a 20-finding sample.
6. Languages. Recommended: Rust and Python only in phase 1, by the `callee_stoplist` key.
7. Rows that `clone_delta` also gives. Recommended: no suppression in phase 1. Measure the overlap, then decide.
8. The `--path` hook form. Recommended: not in phase 1 (stale-cache risk).
9. GF1. Recommended: build phase 1 now, but run the Rust measurement only on a build with GF1. Run Python first.
10. Facts version bump. Recommended: accept one re-index of every cache for the `protocol` and `test` facts.

## 10. Owner answers (2026-11-03)

The owner accepted every recommended answer of section 9, and kept bar A at a false-positive rate of 40% or less
and bar B at 10% or less (Wilson upper bound 20% or less, 60 or more findings, planted recall 50% or more).
