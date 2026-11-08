# Design: duplicate signals before, at and after a write

Status: design for the owner's review, 2026-11-08, `feat/trace-tool` at 2e87419 (brief DUPD). No code changes in
this phase. The measurement scripts and raw results are in `/var/tmp/claude-rust-port/dup/` (`scripts/`, `data/`).
Every count below names its source there.

## 1. The answer in brief

- **No signal that ships today finds the duplicates of this repository.** On the 26 labelled duplicates, `clone_delta`,
  `near_duplicates` and `callee_overlap` each find 0 (Wilson 95% 0-12.9%). All 26 are Type-3 or Type-4 (section 3).
- **`similar` is not a duplicate signal.** The default card shows the partner for 8 of 26 duplicates, but 0 of 15
  sampled function rows of the default card are duplicates (0-20.4%), and 202 of its 701 rows are types or constants.
- **A deterministic pass that agentless can own ranks the partner well.** The IDF-weighted Jaccard of the names that a
  function calls and the fields that it reads (Oreo's "action tokens") puts the partner in the top 3 neighbours for 15
  of 26 duplicates. Its AUC on the labelled pairs is 0.854 (0.756-0.934), against 0.80-0.81 for the jina-v5 embedding
  on the same pairs. Its global precision is low: 9 of its top 45 pairs are duplicates (20.0%, 10.9-33.8%).
- **The two passes find different duplicates.** 44 of the deterministic top 45 pairs, and all 9 of its duplicates, are
  outside the 629 pairs that the three embedding models report together.
- **Recommendation.** Build the action-token pass as a `lint` check first (review), prove it on a held-out
  repository, then promote it by measured bars: to a `reuse` section of the symbol card in place of `similar`, and to
  an advisory `erosion` row on each added function. Keep `clone_delta` in the hook as it is. Keep `near_duplicates` and
  `callee_overlap` at review. Keep the embedding pass outside agentless, as an optional review-skill step.

## 2. What exists, read in the code (2e87419)

| Signal | Rule | Where it runs | Code |
|---|---|---|---|
| `clone_delta` | a block (function or block node) of 3+ SLOC whose Type-2 key (identifiers numbered by first use, literals by class) has another instance; 40+ leaf tokens | `lint --path` and `--since`; the `erosion` hook (advisory) reads the blocks from the tag cache | `crates/metrics/src/clones.rs:35-94`, `crates/checks/src/copies.rs:15`, `crates/checks/src/clone.rs:28` |
| `near_duplicates` | the sha256 of the body after the declaration line, dedented, comments dropped; **identifiers as written** (`syntax::tokens` keeps leaf text); 12+ tokens | `lint --candidates` and `--diff` (patch forms) only | `crates/patchcheck/src/duplicates.rs:16,143-163`, `crates/patch/src/keys.rs:20` |
| `callee_overlap` | f calls 80%+ of the 3+ counted callees of g and does not spell g; a callee counts only at a binding tier, only as a repository function, never a test or a stoplist name | `lint --since` with `--only` only; refused in `--path`, so not in the hook | `crates/app/src/callees.rs:82-110`, `crates/app/src/registry.rs:127-131` |
| `similar` | symbols that 2+ callers of the target also reference, scored by file votes; the default card shows 3 rows at a binding tier | every symbol card; `refs --shared-callers` | `crates/app/src/trace/symbol.rs:248-292`, `crates/app/src/refs/shared.rs:31` |

Correction to the brief: `near_duplicates` is a Type-1 body match, not a normalized one. A renamed local stops it.

## 3. Accuracy, measured

**Data.** The 330 blind labels of 2026-11-02 (26 duplicate, 292 related, 12 unrelated) on the tracked files of
9c8b293. They were in another session's tmpfs scratchpad; a copy is in `dup/data/labels/` (`key.json`,
`final.json`). The label tree is `git archive 9c8b293` in `dup/repos/lab`; it is byte-equal to the labelled export.
The pairs are embedding candidates, so every rate on them is range-restricted (section 5).

**Method.** All runs use the installed CLI (`~/.local/bin/agentless-mcp`, the deployed trace build), a cache under
the work folder, and no model.
- `clone_delta`, `callee_overlap`: for each pair and each side X, a commit object whose tree lacks X (the labelled
  lines and the symbol span), then `lint --since <it> --only callee_overlap` (660 runs, `pair_lint.py`). A hit is a
  row on X that names the other side. A whole-tree run (`--since` an empty root) gives every `clone_delta` copy.
- `near_duplicates`: for each side of each duplicate, a working tree without X and `lint --diff` that adds X (52
  runs, `nd_runs.py`).
- `similar`: `trace <path:line>` and `--more similar` for each of the 534 labelled functions (`similar_runs.py`).
- New labels: 83 pairs in two blind batches (two agents, no source, shuffled, A and B swapped at random): 45
  deterministic top pairs, 15 card neighbours, 15 `similar` rows, 8 controls with known labels
  (`build_packets.py`, `label_key.json`, `labels_new{1,2}.json`). The controls agree on 6 of 8. The 2 differences are
  partial duplicates that the new labellers called related with low confidence, so the new rates may be low.

| Signal | Duplicates found (26) | On the 304 others | Precision on its own output |
|---|---|---|---|
| `clone_delta` | 0 (0-12.9%) | 0 | none to measure: the whole tree holds no Type-2 block copy of 40+ tokens (`observed.clone_instances` is empty). Control: a planted renamed copy and an edited copy give 2 rows |
| `near_duplicates` | 0 of 52 runs (0-6.9%) | not run; no labelled pair has equal bodies by a token scan | not measured |
| `callee_overlap` | 0 of 52 runs | 0 of 608 runs name the partner; 9 runs give a row that names a third function | 13 of 60 (21.7%, 13.1-33.6%), `docs/analysis/2026-11-04_callee-overlap-measurement.md` |
| `similar`, default card | shown for 8 (30.8%, 16.5-50.0%); listed anywhere for 9 | shown for 43 of 292 related (14.7%) and 2 of 12 unrelated | 0 of 15 function rows (0-20.4%) |
| action tokens (prototype) | top 1: 9; top 3: 15 (57.7%, 38.9-74.5%); test and protocol functions left out | not a pair rule | top 45 pairs: 9 (20.0%, 10.9-33.8%); top neighbour at score 0.3+: 2 of 15 (13.3%, 3.7-37.9%) |

**Per clone type.** A token scan types each pair by the BigCloneBench bands (similarity after Type-2 normalization;
derived, `clone_types.py`): 0 Type-1, 0 Type-2, 1 VST3, 1 ST3, 8 MT3, 16 WT3/T4. `similar` shows 4 of the 10 MT3-or-
better pairs and 4 of the 16 WT3/T4; the action-token top 3 holds 7 of 10 and 8 of 16.

**What each misses, and why.**
- `clone_delta` and `near_duplicates` match exact hashes, and no duplicate here is a Type-1 or Type-2 copy. The
  repository gate keeps the tree free of Type-2 copies, so these checks guard against new copies only.
- `callee_overlap` counts only edges to repository functions at a binding tier. These duplicates share standard-
  library calls (`iter`, `position`, `read`, `from_utf8`) and method calls on a receiver, which never bind (design D2
  of `docs/plans/2026-11-02_callee-overlap-duplicate-check.md`). Example: `rationales::owner` and
  `owners::enclosing_symbol` share no counted callee.
- `similar` ranks what the target's callers also use. Its rows are mostly co-used types (`Symbol`, `NodeId`). It shows
  a duplicate only when one caller references both twins, as with two test helpers of one test file.
- The action-token pass misses 11 of 26: 4 pairs with a side under 3 action tokens, pairs whose shared names are all
  common (`iter`, `new`, `to_owned`; more than 5% of functions), and 1 pair that shares no name (`P169`). Its top
  pairs are mostly same-file siblings of 3 to 5 lines (accessor families).

## 4. The surface

| Signal | Before writing (card) | At writing (`erosion`) | At review (`lint`, skills) |
|---|---|---|---|
| `clone_delta` | no: no Type-2 copy exists to show | keep, advisory | keep |
| `near_duplicates` | no | no: the hook form is the pair form; `clone_delta` covers copies of 40+ tokens | keep (patch forms) |
| `callee_overlap` | no: 0 of 26, 21.7% on its own rows | no: fails bar A (78% false positives), and needs a whole-tree graph in a new process | keep, opt-in |
| `similar` | move to `more=similar` only: 0 of 15 function rows are duplicates | no | `refs --shared-callers` stays |
| action tokens (new, `reuse`) | the `reuse` section in place of `similar`, after bar C | advisory row per added function, after bar A | new check `reuse_candidates`, opt-in first |
| embedding pass | no: agentless calls no model | no | optional `/architect-review` step through the docs server |

A search hint and a file-card line are rejected: no measurement supports them, a search answer is a literal-match
list, and a file-card line at the 0.3 score floor would fire on 33% of functions (`open_search.py`).

**Latency** (`latency.py`; installed build, warm tag cache, a new process each run, 7 runs, median and maximum;
druid is Java and has no callee stoplist, so no tree check runs there):

| Placement | this repository (953 files) | sympy (1,511 .py) | druid (8,186 .java) |
|---|---|---|---|
| symbol card, CLI `trace` (refs and explain scans) | 0.20 s, 0.22 s | 0.55 s, 0.56 s | 3.23 s, 3.25 s (1.1 GB) |
| the hook form today, `lint --path --after` with `clone_delta` from the cache | 0.017 s, 0.023 s | 0.056 s, 0.060 s | 0.18 s, 0.19 s |
| a tree check, `lint --since HEAD --only callee_overlap`, one added function | 0.44 s, 0.44 s (287 MB) | 0.98 s, 1.02 s (431 MB) | not run |

The `erosion` deadline is 10 s less a 0.5 s log reserve (`crates/hook/src/call.rs:17,19`); `read-card` measured 448
ms warm p95 and 825 ms cold (brief). Placement rule: a hook row reads stored per-function data from the tag cache, as
`clone_delta` does; it never builds the symbol graph in the hook (0.44 s here, about 3 s on druid by the card cost).
The `reuse` pass then costs one inverted-index probe per function (derived; the Python prototype loads 4,314
functions and scores all their pairs twice, for two filter sets, in 1.4 s). In the server, the index is built once
per tree generation.

## 5. The gap: Type-3 and Type-4

| Evidence | Deterministic action-token pass | Embedding pass (jina-v5, outside agentless) |
|---|---|---|
| AUC on the 327 mapped labelled pairs (duplicate against not) | 0.854 (0.756-0.934); calls only 0.833; identifier subwords 0.789; token 4-grams 0.697 (`det_pass.py`) | 0.802 cached vectors, 0.814 re-embedded (2026-11-02 research notes) |
| Precision of the global top 45 | 20.0% (10.9-33.8%); cross-file pairs only: 7 of 16 (post hoc) | 39% (26-53%), cross-file by its input |
| Overlap | 1 of 45 top pairs and 0 of its 9 duplicates in the embedding union | 629 pairs in the union |
| Cost and ownership | in-process, deterministic, no dependency; reads stored names | a model server (port 8085), 108 s per full scan, CC-BY-NC weights |
| Per-target use (card, hook) | yes | no: agentless never calls a model |

**Decision.** Build the deterministic pass in agentless. It is the only option for the card and the hook, it
separates duplicates from related siblings at least as well as the embedding on the same pairs, and it finds
duplicates that the embedding union does not hold. Keep the embedding pass as an optional review step: at equal
volume its global list is about twice as precise. The evidence does not decide the global review list: n is small
(26 + 9 + 2 duplicates), agents labelled every pair, one repository, and the best of 4 scores was chosen after the
fact. The measurement that decides: blind labels, by this protocol, of both passes' top 45 cross-file pairs on a
held-out repository (one Rust, one Python), with the success bar written before the run.

## 6. Thresholds and noise

About 88% of the embedding candidates were related siblings; the action-token top 45 is 76% related. Each placement
keeps that noise from the agent as follows:
- **Filters, as data with one home.** Leave out protocol methods and test functions (the `marks` facts that
  `callee_overlap` uses; research K9 found that the same-trait filter removes 37 related pairs and no duplicate).
  Leave out a candidate that the function calls or that calls it, by resolved edges, never by name (K9: a name match
  dropped 9 of 26 duplicates). Count a name only when its document frequency is at most 5% of functions, and require 3
  or more action tokens on both sides (`open_search.py` constants).
- **Cross-file only on the card and in the hook.** A same-file twin is in view when the agent reads the file. In the
  new labels, same-file pairs hold 2 duplicates in 34 (top 45 and card samples); cross-file pairs hold 9 in 26
  (34.6%, 19.4-53.8%). All 26 labelled duplicates are cross-file. This filter is post hoc: the held-out run tests it.
- **A floor from held-out labels.** On this repository, a 0.3 floor shows a row for 33% of functions and keeps 7 of
  the 26 duplicates; 0.4 shows 19% and keeps 3. The rank works better than the score (15 of 26 in the top 3), so the
  floor must come from labels, not from these figures. Open value, set by bar C.
- **Wording.** A row names a candidate to read, never a verdict: "`load_text` @path:line calls and reads 6 of the
  same names (`stat_present`, `read_bounded`, ...). Read it before you keep a second copy." At most 2 rows per card,
  1 per added function in the hook, with the shared names so that the agent can judge without a call.
- **Review only for the rest.** `callee_overlap`, the same-file pairs and the embedding list go to `lint` and the
  review skills, where a verifier (an LLM reader) reads each row.

## 7. Proof plan

1. **Planted fixtures.** Extend `tests/characterization/fixtures/callee_overlap_rust` and `_py` (research 3.3), or add
   `reuse_rust` and `reuse_py`: per language 3 renamed copies, 3 edited copies, 4 re-implementations that share only
   standard-library and method calls (the shape `callee_overlap` misses, such as `owner` and `enclosing_symbol`), and
   8 negatives (a same-file accessor family, a sibling pipeline, a protocol method, a test, a caller-callee pair, a
   moved function, a function under 3 action tokens, a cross-language pair). A `lint` golden per fixture; the test
   asserts every row is a planted positive. Predict each golden before the run.
2. **Goldens for the card.** `tests/characterization/goldens/trace/` gains a symbol card with a `reuse` section and
   one with `reuse: none`; the grammar row is `id " @" span "  " shared " shared names: " name { ", " name }`. Write
   the expected text first, then compare. Update the cl100k counts in `crates/app/fixtures/read/tokens.json`.
3. **Held-out check.** Label blind, with this protocol, the top 45 cross-file pairs of the deterministic pass and 40
   card rows (seeded functions) on two repositories that no setting saw: one Rust (`tokio-rs__tokio-6752`) and one
   Python (`sympy__sympy-24661`, from the frozen bench copies, copied first). 8 controls per batch. The 330 labels of
   this repository stay a test set; no setting is tuned on them again.
4. **Bars, written before the held-out run.** Bar C (card): precision of 30% or more over 40 or more card rows, Wilson
   lower bound 15% or more, and at most 1 card in 4 with a row. Bar A (hook advisory): the owner's bar for
   `callee_overlap`, 40% false positives or less over 40 or more rows. Block mode is not in scope.
5. **Latency.** The `latency.py` table again on the build, with the `reuse` section and the hook row. The card may add
   at most 10% to the symbol-card time in a new process; the hook row at most 50 ms at p95 on druid.
6. **Trial metric.** The hook log gains, per `reuse` row shown, the session, the added function and the candidate.
   `gate-report` then reads the final tree of each session: a row counts as acted on when the added function is gone
   or calls the candidate. The owner sees three counts per trial window: rows shown, rows acted on, and duplicates
   introduced per 100 added functions (the `reuse_candidates` check on the window's net diff, read blind), against the
   same counts for the window before the trial.

## 8. Build phases

One builder per phase, disjoint file sets, every gate of `AGENTS.md` green, `scripts/gate-selftest.sh` for a phase
that changes `xtask/` or a policy file. Estimates are tokens of builder work, derived from the P1 to P4 phases.

| Phase | Owns | Estimate | Gate to start the next |
|---|---|---|---|
| R0, PM and owner | the open points; the bars of 7.4 written down; the label set moved to a kept place | none | owner's word |
| R1, stored names | per-function action names (call and field position) computed in the clone walk (`crates/metrics/src/clones.rs` or a sibling module), stored in the tag cache; the cache version bump; `crates/metrics/fixtures/` | 150k | unit tests; one re-index of every cache, predicted |
| R2, core and lint | `crates/checks/src/reuse.rs` (pure: IDF weights, weighted Jaccard, inverted index, filters), `crates/app/src/reuse.rs` (shell: cache read, resolved edges), the pair-table row `reuse_candidates` (`--since` and `--path`, opt-in), prompts rows and `render.json` fixtures, the planted fixtures and `lint` goldens, `capabilities`, the guide and CHANGELOG | 200k | planted recall 10 of 10 per language, 0 rows on negatives |
| R3, measurement | none in the tree; `docs/analysis/<date>_reuse-measurement.md` | 100k + 2 labelling agents (about 140k) | bar C and bar A read; stop and report if neither passes |
| R4, card | `crates/app/src/trace/symbol.rs` and `section.rs` (`reuse` on the default card, `similar` only under `more=`), `crates/render/src/trace*`, prompts, trace goldens, `tokens.json` | 150k | bar C passed; surface diff of the trace cards |
| R5, hook | `crates/hook/src/gate.rs` and its record, `GatedCheck` in `crates/io/src/config.rs`, `.agentless-mcp.json`, hook goldens, `gate-report` counts of 7.6 | 120k | bar A passed; latency of 7.5 |
| R6, trial | none; the owner's environment, on the owner's word | none | the counts of 7.6 |

R4 and R5 can run at the same time on their disjoint sets. A function above complexity 10 that a phase must branch
gets its refactor commit first, with no change in behavior.

## 9. Open points for the owner

1. **`similar` on the default card.** The trace design (section 2.1) keeps L4 because `similar` "serves duplicate
   avoidance". Measured: 0 of 15 function rows are duplicates. Recommended: L3 as the default until R4 lands, then
   `reuse` in the place of `similar`; `similar` stays under `more=similar`. This changes the card contract.
2. **Bars C and A** (section 7.4): confirm 30% precision for the card and the existing 40% false-positive bar for the
   hook, or set others.
3. **The cross-file rule** on the card and in the hook: accept it as a filter keyed on what the agent can see, to be
   tested on the held-out labels.
4. **The label set.** It lives only in another session's tmpfs scratchpad and in `dup/data/labels/`. Recommended: keep
   it with the measurement records outside the shared checkout, as a frozen test set.
5. **Held-out repositories and labelling cost**: tokio and sympy, about 140k tokens of labelling agents per round.
6. **The embedding step**: keep it as an optional `/architect-review` step, or drop it; agentless needs no change
   either way.
7. **`near_duplicates` naming**: its message says "normalises to the same token stream", but it matches identifiers as
   written. A one-line message change or a rename, separate from this work.
