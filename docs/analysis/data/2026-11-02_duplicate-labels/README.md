# Duplicate labels of 2026-11-02: the frozen test set

These files are the labelled function pairs that the duplicate-logic design
(`docs/plans/2026-11-08_duplicate-logic.md`, section 3) measures against. No setting is tuned on them again
(section 7.3): they are a test set.

## The tree

Every span is a repository-relative path with a first and a last line in the tracked files of commit 9c8b293
(`git archive 9c8b293`).

## The 330 pairs of 2026-11-02

- `key.json`: the two spans of each pair id (`P001` to `P330`), the region of each pair, the sampled and
  double-labelled pairs, the batches, the quotas and the region sizes.
- `big_sample.json`: the same 330 pairs as rows: the pair number, an empty field, and the two spans.
- `labels1.json` to `labels6.json`: the six labelling batches.
- `final.json`: one label per pair: 26 `duplicate`, 292 `related`, 12 `unrelated`.

Selection: three embedding models (jina-v5-text-small retrieval, CodeRankEmbed, jina-code-1.5b) reported 629
distinct pairs together. The union was split into seven regions by which models report a pair. Every pair of the two
small regions, a seeded random sample of the five large ones, and the pairs of an earlier single-model run gave the
330 pairs. Because every pair is an embedding candidate, every rate measured on them is range-restricted.

Protocol: six agents labelled the pairs blind (no model name, no score, A and B order shuffled). 30 pairs went to two
agents, with 100% agreement on duplicate against not. Agents labelled every pair; no person did.

## The 83 pairs of 2026-11-08 (`2026-11-08_dupd/`)

- `label_key.json`: one row per pair: its id (`Q001` onward), its source, and the two spans `a` and `b`.
- `labels_new1.json`, `labels_new2.json`: two blind batches, one label, a confidence flag and a reason per pair.

The pairs are 45 top pairs of the action-token prototype, 15 of its card neighbours, 15 `similar` rows and 8
controls with known labels. Two agents labelled them blind, with no source, shuffled, and A and B swapped at random.
The controls agree on 6 of 8. The 2 differences are partial duplicates that the new labellers called related with low
confidence, so the rates from these batches may be low.
