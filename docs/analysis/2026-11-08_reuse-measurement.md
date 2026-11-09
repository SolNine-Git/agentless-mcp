# reuse_candidates: the held-out measurement (R3)

This record holds R3 of the duplicate-logic design ([the design](../plans/2026-11-08_duplicate-logic.md), sections
7.3 and 8). It labels the rows of `lint`'s `reuse_candidates` check on two repositories that no setting saw. Both bars
fail. By the R3 gate, R4 (the card) and R5 (the hook row) do not start, and the check stays opt-in at review.

## Verdicts

| Bar (as fixed) | Measured, both repositories | Verdict |
|---|---|---|
| C (card): precision 30% or more over 40 or more card rows, Wilson lower bound 15% or more, at most 1 card in 4 with a row | precision 14 of 113 rows, 12.4% (7.5-19.7%); cards with a row 62 of 80, 77.5% (67.2-85.3%) | **fails** all three conditions; n passes |
| A (hook advisory): at most 40% false positives over 40 or more rows | false positives 99 of 113 card rows, 87.6% | **fails** |

Each repository also fails each bar on its own: tokio 6 of 58 (10.3%, 4.8-20.8%), 32 of 40 cards with a row; sympy 8
of 55 (14.5%, 7.6-26.2%), 30 of 40 cards.

## Method

- **Bars first.** [`bars.md`](data/2026-11-08_reuse-labels/bars.md) fixes the bars and the reading rules at
  2026-11-08 18:56 EST. The labels did not exist then; the label files date from 19:06.
- **Build.** `feat/trace-tool` at ca23423, release build, sha256 `cc9d6a71de0e...`, frozen in the work folder. The
  check runs as built: floor 0.3, 3 or more action names, a counted name in at most 5% of functions, at most 2 rows
  per function (`crates/checks/src/reuse.rs:33-38`).
- **Trees.** Copies of the bench snapshots `tokio-rs__tokio-6752` (6fed0fa, 741 files) and `sympy__sympy-24661`
  (2ce3053, 1,527 files), index refreshed. Every run: `lint --since <rev> --only reuse_candidates --json`.
- **Global list.** One run since an empty-root commit, so that every function is added: 1,148 rows on tokio (600
  cross-file) and 8,871 on sympy (3,257 cross-file). The top 45 cross-file rows by score per repository. All of them
  score 1.000, with 155 (tokio) and 190 (sympy) cross-file rows tied there, so a seeded key breaks the tie: the list
  is a random sample of the score-1.000 pairs.
- **Card rows.** The frame is the check's own `kept` set: functions in metric scope with 3 or more action names and
  no test or protocol mark (1,417 of 4,195 functions on tokio, 10,366 of 19,402 on sympy). 40 functions per
  repository in seeded order (seed 20261108). For each one, a child commit of HEAD whose copy of the file lacks the
  function's span is the base, so that the function is the one added function. Its rows are the card rows: 58 on
  tokio, 55 on sympy. No run gave a `not_checked` row.
- **Packets.** The DUPD form: no score and no check name, A and B swapped by a fixed seed, shuffled, each pair as
  its two spans with their source text. Distinct pairs: tokio 97 (4 pairs are in both sets, and 2 card rows repeat
  the pair of another card), sympy 100. Each repository has 2 batches, with 20 pairs in both, and each batch has 8
  controls from the 2026-11-02 labels (4 `duplicate`, 4 `related`, 32 distinct pairs). Labeller 1 did batch 1 of
  both repositories, and labeller 2 did batch 2. The definitions given to both: `duplicate` is one rule or job in two
  homes that must change together (a partial case is `duplicate` with `low_confidence`); `related` is the same family
  or pattern with separate jobs; `unrelated` has no shared job or pattern.
- **Scoring.** By `bars.md`: the owner batch's label counts; the second only toward agreement. Wilson 95%. Scripts:
  `/var/tmp/claude-rust-port/dup3/scripts/` (`frame.py`, `card_rows.py`, `build_packets.py`, `score.py`).

## Results

| Read | tokio | sympy | Both |
|---|---|---|---|
| Card rows, precision | 6/58, 10.3% | 8/55, 14.5% | 14/113, 12.4% (7.5-19.7%) |
| Cards with a row | 32/40, 80.0% | 30/40, 75.0% | 62/80, 77.5% (67.2-85.3%) |
| Cards with a row, over all functions in metric scope (derived) | 27% | 40% | |
| Card rows, false positives (bar A) | 52/58, 89.7% | 47/55, 85.5% | 99/113, 87.6% |
| First-ranked row of each card | 4/32, 12.5% | 8/30, 26.7% | 12/62, 19.4% (11.4-30.9%) |
| Global top 45 (all score 1.000) | 9/45, 20.0% (10.9-33.8%) | 12/45, 26.7% (16.0-41.0%) | 21/90, 23.3% (15.8-33.1%) |
| Double-labelled pairs, same label | 18/20 (kappa 0.62) | 19/20 (kappa 0.86) | 37/40 |
| Controls, same as the known label | 16/16 | 15/16 | 31/32 |

- **Low confidence.** 7 of the 14 card-row duplicates are partial duplicates (`low_confidence`). With them counted as
  not duplicates, card precision is 7 of 113 (6.2%, 3.0-12.2%). 2 of the 21 global duplicates are partial.
- **Controls.** The one miss is P308 (known `related`), labelled `duplicate` with low confidence by labeller 1. 12 of
  the 16 duplicate controls have low confidence. **Agreement:** all 3 disagreements are `duplicate` against
  `related`; read them with the caveat under Limits.
- **This repository** (design section 3) gave 9 of 45 global pairs (20.0%) and 2 of 15 top card neighbours (13.3%).

## The cross-file rule

The design keeps only cross-file rows on the card and in the hook (section 6). The rule was post hoc: on this
repository, cross-file pairs held 9 duplicates in 26 and same-file pairs 2 in 34. The held-out labels do not repeat
that difference:

| Card rows | tokio | sympy | Both |
|---|---|---|---|
| Cross-file | 4/33, 12.1% | 2/22, 9.1% | 6/55, 10.9% (5.1-21.8%) |
| Same-file | 2/25, 8.0% | 6/33, 18.2% | 8/58, 13.8% (7.2-24.9%) |
| Cards with a cross-file row | 20/40 | 15/40 | 35/80, 43.8% (33.4-54.7%) |

Fisher's exact test, cross-file against same-file: p = 0.78. With the rule, bar C still fails (10.9% precision, 43.8%
of cards with a row) and bar A fails (89.1% false positives). The rule does not earn its place on these labels.

## Exploratory reads, for a held-out re-test

These reads choose among settings after the labels were read. They are not a bar and not a result. Any setting they
suggest needs new repositories and bars fixed before the labels.

**(a) Score 1.000.** The 101 distinct labelled pairs at score 1.000 hold 21 duplicates (20.8%, 14.0-29.7%): 21 of
the 90 global pairs, and 0 of the 15 card rows at 1.000 (0-20.4%). A score of 1.000 means that both functions have
the same set of action names. 12 of the 15 card pairs at 1.000 are delegate wrappers whose one distinct call has the
wrapper's own name, such as `rename` around `std::fs::rename` and `to_ring` around `f.rep.to_ring`. The action-name
pass never counts the unit's own name (`crates/metrics/src/names.rs:55-77`), so each wrapper family scores 1.000
against itself. In the global top 45, pairs where both sides call their own name hold 2 of 28 duplicates (7.1%) and
the others 19 of 62 (30.6%). The own-name test is a regular expression over the span text (derived).

**(b) Higher floors and a minimum count of shared names.** Card rows of both repositories: duplicates over rows, and
the cards with a row out of 80.

| Floor | any shared count | 3 or more shared | 4 or more shared | 5 or more shared |
|---|---|---|---|---|
| 0.3 (as built) | 14/113 12.4%, 62 cards | 14/99 14.1%, 55 | 10/61 16.4%, 36 | 6/36 16.7%, 20 |
| 0.4 | 13/91 14.3%, 53 | 13/81 16.0%, 48 | 10/49 20.4%, 29 | 6/30 20.0%, 17 |
| 0.5 | 11/71 15.5%, 43 | 11/67 16.4%, 40 | 9/41 22.0%, 23 | 6/24 25.0%, 13 |
| 0.6 | 10/56 17.9%, 36 | 10/54 18.5%, 34 | 8/36 22.2%, 23 | 5/21 23.8%, 13 |
| 0.7 | 9/39 23.1%, 27 | 9/38 23.7%, 26 | 8/26 30.8%, 18 | 5/16 31.2%, 11 |
| 0.8 | 2/22 9.1%, 14 | 2/22 9.1%, 14 | 1/14 7.1%, 9 | 1/8 12.5%, 6 |
| 0.9 | 0/17, 9 | 0/17, 9 | 0/10, 5 | 0/4, 2 |
| 1.0 | 0/15, 8 | 0/15, 8 | 0/8, 4 | 0/2, 1 |

- Precision does not rise with the score. Rows from 0.7 to under 0.8 hold 7 of 17 duplicates (41.2%), and rows at
  0.8 or more hold 2 of 22 (9.1%). The wrapper families of (a) fill the top band.
- No cell meets every condition of bar C. The best cell, floor 0.7 with 4 or more shared names, has 8 of 26 (30.8%,
  16.5-50.0%) with 18 of 80 cards (22.5%). n is under 40, the cell is the best of 32, and it splits 5 of 14 on tokio
  and 3 of 12 on sympy.
- A floor reads exactly, because a card shows its best rows first. A shared-name minimum does not: the filter could
  let a third-ranked row onto a card, and the data hold only the top 2. Under that filter, the rows and the share of
  cards are lower bounds.

## Decision for R4 and R5

- **R4 (the card `reuse` section): do not start.** Bar C fails on precision, on its lower bound and on the share of
  cards. 3 cards in 4 would show a row, and about 7 rows in 8 would not be duplicates.
- **R5 (the hook advisory row): do not start.** Bar A fails at 87.6% false positives against a 40% ceiling.
- **`reuse_candidates` stays an opt-in `lint` check**, where a reviewer reads each row. The floor stays provisional.
- **Next measurement, if the owner wants one.** Two candidate changes from the exploratory reads: count a qualified
  call to the unit's own name (the delegate-wrapper case of (a)), and a higher floor with a shared-name minimum (b).
  Each needs bars fixed first and two new held-out repositories, because these labels chose them.

## Limits

- **Labeller incident.** The two labellers shared one scratch folder. Labeller 1 by mistake ran labeller 2's script,
  which rewrote the batch 2 files from labeller 2's own table. The PM checked that the batch 2 files equal that table
  for all 134 items. No evidence shows whether labeller 1 saw labeller 2's labels, so the agreement of 37 of 40 can be
  high. Every rate above counts one label per pair, but the batch 1 label of a double-labelled pair can still depend
  on its batch 2 label.
- **Agents labelled every pair; no person did.** The labellers saw the source text and no score, check name or
  source set.
- **Controls are foreign.** They come from agentless-mcp at 9c8b293, so a labeller can tell them from the tokio and
  sympy pairs, though not their known labels.
- **Small and correlated.** Two repositories, 40 cards each. A card gives up to 2 rows, so the rows of one card are
  not independent, and the Wilson intervals on rows are somewhat too narrow.
- **The frame is the check's own.** It keeps helpers in a Rust `mod tests` that have no test mark (such as
  `tests.collect`). A card for a function outside the frame never gets a row; the derived shares count those cards.
- **The card is simulated.** A card row is the check's row for the one added function. A card that applies the
  cross-file rule could show a third-ranked cross-file row that the check never lists. The global list is a seeded
  sample of tied pairs, because the score cannot rank its top.

## Files

[`data/2026-11-08_reuse-labels/`](data/2026-11-08_reuse-labels/): `bars.md`; `label_key.json` (spans, sources,
scores, owner batch, known labels of the controls); `cards_*.json` (the 40 sampled functions with their rows, empty
cards too); `global_*.json`; `labels/`; `scores.json` (every figure here). The labelled packets quote tokio and sympy
source verbatim, so they stay outside this repository. `build_packets.py` rebuilds them from `label_key.json` and the
two snapshot commits named under Trees.
