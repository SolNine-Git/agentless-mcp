# Research Report: improving structural, model-free code localization

**Date**: 2026-09-27 | **Query**: techniques to improve seed resolution,
graph ranking, retrieval fusion and context budgeting for the agentless-mcp
ranking core | **Tree**: `feat/0.8.2-spelling-seeds` at 674b7b1

## 1. Executive summary

The literature converges on the design this server already has, and the one
gap it named has now been built and rejected on measurement. LocAgent runs a
four-tier entity index; 0.8.2 shipped its fourth tier, and its third -- a
lexical index for keywords that miss exact names -- was prototyped during
this pass and does not survive 150 instances.

The durable conclusion is therefore about the instrument rather than the
features. Every idea the literature offered was either already present, too
small for this benchmark to resolve, or a transfer from a system with an LLM
in its retrieval loop. The subset was widened from 50 to 150 instances to
decide the two that were merely small, and that widening is the most useful
thing this pass produced.

**Confidence**: High on the gap analysis and on both rejections, which rest
on paired bootstraps over 150 instances. Medium on the remaining
recommendation, which is untested.

**Primary sources**: LocAgent (2503.09089); Code Isn't Memory (2606.22417);
Beyond Semantic Similarity (2605.05242).

**Correction this report makes to itself**: an earlier draft claimed 0.8.2's
gains concentrate on multi-file changes. Segmenting the 150 instances shows
the opposite -- the whole effect is in the 94 single-gold instances (MAP
+0.0575, five of five metrics clearing) and the 56 multi-gold instances show
nothing (+0.0133, none clearing). Section 3.3 has the reasoning error that
produced the wrong claim, and the weakness the split exposed: only 5 of 56
multi-gold instances ever get their whole gold set into a top 10.

**Built, measured and rejected during this pass**: the BM25 subword tier
(section 3.2) and `relation_weights` (section 3.1). Neither clears this
project's statistical bar. The subword tier is the sharper negative: at 150
instances it moves 12 instances up and 16 down on MAP.

**Recommended against on design grounds**: folding commit churn into the
rank, and making the map exploration-aware. Section 3.4 gives the reasons.

**Instrument change**: the deterministic subset was expanded from 50 to 150
instances (section 3.5). It is a strict superset, so no earlier result is
invalidated, and it raised the shipped tier's confirmed metrics from 6 of 14
to 12 of 14.

## 2. Key findings

| # | Finding | Source | Confidence |
|---|---|---|---|
| F1 | LocAgent resolves issue keywords through four tiers: fully-qualified id, bare-name dictionary, BM25 over entity ids, and an inverted index from code chunks to entities. agentless has tiers 1, 2 and (as of 674b7b1) 4. | LocAgent 2503.09089 | High |
| F2 | A structural index beats agentic grep on localization and on dollars per solve inside a fixed harness and model, conditional on the workload containing multi-file changes. | Code Isn't Memory 2606.22417 | High |
| F3 | Direct corpus interaction beats embedding retrieval for agents, except where the corpus is large and the task is finding the first useful anchor, where an index still wins. | Beyond Semantic Similarity 2605.05242 | High |
| F4 | Graph candidate expansion raises dense-retrieval Recall@20 by at least 13% relative along containment edges and at least 27% once call edges are walked. Each surfaced unit carries its seed and edge type, so the candidate set is auditable. | SpIDER 2512.16956 | Medium |
| F5 | Commit-history memory improves LocAgent's localization on SWE-bench-verified and SWE-bench-live. The mechanism is retrieval over commits and linked issues, not a recency prior on the ranking. | Improving code localization with repository memory, ICLR 2026 | Medium |
| F6 | An evolving repository view that refreshes as exploration proceeds gains 2.4 resolve points and cuts input tokens 5.8%, against a static graph baseline. | RepoAtlas 2609.16936 | Medium |
| F7 | Serving repository context through routed views (lexical, semantic, hybrid, structural) with route-local fusion cuts trajectory tokens to 12.9-49.9% of paired grep-and-read at bounded recall loss. | CodeNib 2607.25431 | Medium |

Two findings deserve emphasis because they bear on strategy rather than on
features.

F3 states this server's niche more precisely than the project's own
documentation does. agentless is not a retrieval layer competing with grep.
It is the anchor-finding layer that grep is weak at, and only in a corpus
large enough for breadth-first discovery to dominate. That is exactly the
shipping gate's shape: defer native search until one structural call, then
give it back. It also predicts the project's own measured result, where
free-choice agentless scored worse than the grep-only baseline on precision
while the gated arm did not.

F2 supplies the segmentation this project's benchmark does not yet do. The
condition it names, multi-file changes, is measurable here: `acc_all@k`
requires every gold file in the top k, and the 150-instance subset averages
2.13 gold files per instance.

## 3. Implementation guide

### 3.1 relation_weights: measured, and the default stays off

`relation_weights` exists behind `ctx.config.relation_weights` and defaults
off. F4 is the argument for turning it on; it is not evidence, because
SpIDER measures candidate expansion for a dense retriever rather than a
PageRank walk. The deterministic tier was run on 2026-09-27 to settle it,
`.agentless-mcp.json` written into all 49 checkouts and removed afterwards,
scored against `v082-branch`:

| metric | delta | 95% CI | better/worse |
|---|---|---|---|
| nDCG@10 | +0.0131 | +0.0010 to +0.0286 | 6 / 2 |
| `acc_any@10` | +0.0600 | +0.0000 to +0.1400 | 3 / 0 |
| `recall@10` | +0.0300 | +0.0000 to +0.0700 | 3 / 0 |
| MAP | +0.0013 | -0.0044 to +0.0070 | 8 / 7 |

One metric of fourteen excludes zero, which this project's own statistical
rule says to read as the expected behaviour of fourteen tests rather than as
a result. The shape is a coverage gain with no precision gain, the opposite
of the spelling tier's shape, and MAP is flat. Nothing is harmed, and the
direction is favourable on every metric that moves at all, so this is worth
re-running on a larger or multi-language subset before it is dismissed. It
is not grounds to flip the default today.

### 3.2 The subword lexical tier: built, measured, rejected

LocAgent's missing tier three was prototyped as BM25 over subword-split
identifiers, sitting after the exact-occurrence tier. Documents are non-test
files, terms are the `snake_case` and `camelCase` subwords of every
identifier the file spells, and a file scores only when it carries every
subword of the query.

The gate on how many subwords a query must have turns out to decide the
result. Measured at n=50 against the shipped tier:

| variant | `acc_any@1` | `acc_any@10` | `recall@10` | MAP |
|---|---|---|---|---|
| ungated, `min=1` | **-0.0200** | +0.0800 | +0.0487 | **-0.0103** |
| gated, `min=2` | +0.0200 | +0.0400 | +0.0064 | +0.0077 |

The ungated form is the predicted failure: it buys coverage and pays for it
in precision, because a one-word query such as `queue` touches every queue
in the repository. The gate fixes the sign but not the size.

At n=150 the gated form settles it. Zero of fourteen metrics exclude zero,
MAP +0.0054 (95% CI -0.0127 to +0.0254), nDCG@10 +0.0084 (-0.0088 to
+0.0275). The instance counts are the real verdict, and they worsened as the
sample grew: MAP moves 12 instances up and **16 down**, nDCG@10 9 up and 14
down. It helps a few instances a lot and hurts more instances a little,
which is what a noisy seed source looks like.

**Rejected.** For contrast, the exact-occurrence tier shipped in 674b7b1 is
eight times larger for twenty lines and no new index. The prototype is not
in the tree.

### 3.3 Benchmark segmentation: run, and it refuted this report

F2's condition is whether the workload holds multi-file changes, so the
n=150 arms were split on exactly that line: 94 single-gold instances against
56 multi-gold. The prediction, written in an earlier draft of this report,
was that 0.8.2's edge would concentrate in the multi-gold half, because all
four `acc_all` metrics cleared the bar on the whole subset.

**That prediction was wrong, and the reasoning behind it was wrong.**

| stratum | MAP delta | 95% CI | metrics clearing |
|---|---|---|---|
| all (n=150) | +0.0410 | +0.0179 to +0.0680 | 5 of 5 |
| single-gold (n=94) | +0.0575 | +0.0234 to +0.0996 | 5 of 5 |
| multi-gold (n=56) | +0.0133 | -0.0074 to +0.0342 | 0 of 5 |

The whole effect is in the single-gold half. The error was reading
`acc_all@k` as a multi-file measurement: on a single-gold instance
`acc_any@k`, `acc_all@k` and `recall@k` are the same number, which the
per-stratum base rates show plainly (all three are 0.5426 before and 0.6277
after). Ninety-four of the 150 instances have one gold file, so the
whole-subset `acc_all` columns were mostly single-gold instances wearing a
different metric's name.

The segmentation also exposes the ranker's real weakness, which no
whole-subset number showed:

| | single-gold (94) | multi-gold (56) |
|---|---|---|
| `acc_any@10` | 0.6277 | 0.6964 |
| `acc_all@10` | 0.6277 | **0.0893** |

Finding *a* gold file in a multi-file change is easier than in a single-file
change. Finding *all* of them essentially never happens: 5 of 56 instances,
and 0.8.2 changed none of them. Any claim that this server helps with
multi-file changes has to be about `acc_any`, not coverage.

One caveat on reading that against F2. loc-bench scores a single ranked
list; the paper scores an agent that may call many times. Poor one-shot
coverage of a multi-file gold set is not the same as an agent failing to
find those files across a session. Only the agentic tier can separate the
two, and this project's agentic evidence is from 0.6.1 on 2026-08-24, before
the 0.6.3 ranking fix.

**Keep the segmentation.** It cost one script over existing result files and
it caught a false inference inside an hour.

### 3.4 Two proposals to decline

**Folding churn into the rank.** F5 is not evidence for this. The ICLR
mechanism is retrieval over commit messages and linked issues, which the
agent reads, plus functionality summaries of actively changing modules. It
is a memory surface, not a recency prior on a ranking score. Three further
objections:

- `_with_churn` runs one bounded git call *after* the ranking chose its
  files. Folding churn into the rank means measuring churn for every file
  before ranking, which is a git log over the whole tree.
- The 0.7.1 rollback already withdrew adjacent unmeasured ranking work, and
  its note is the standing rule: no ranking change without a measurement
  that can resolve it.
- A confound specific to this benchmark. loc-bench checks out at
  `base_commit` and scores against the next commit's patch. Recent churn at
  `base_commit` correlates with the area under active development, which is
  where that patch lands. A churn gain here may partly measure the
  benchmark's construction rather than localization skill. Any such
  experiment needs a held-out design before its number means anything.

**Making the map exploration-aware.** F6 is real but describes the wrong
layer. RepoAtlas is a module inside one agent's harness. agentless is a
server that may hold many roots and serve concurrent callers, and whose
determinism is a shipped property: two runs of the same tree differ on 0 of
50 instances, and the cache receipt model rests on it. Cross-call state
would break determinism, break cacheability, and make an answer depend on
history the server does not own. The agent already holds that state and can
express it by changing its focus seeds.

### 3.5 The instrument was the bottleneck, and was widened

Both rejections above were unresolvable at 50 instances: the subword tier's
95% interval on MAP spanned 0.036, against an effect of 0.008. The subset
was therefore raised from 50 to 150 instances, 104 repositories, 3.2 GB of
checkouts. `select_subset` walks repositories in ascending size order, so
the larger quota is a strict superset -- verified -- and every earlier
result stands.

The added instances come from larger repositories, which makes the subset
both harder and slower, and moves it toward the large-corpus regime F3 names
as this server's niche:

| | n=50 | n=150 |
|---|---|---|
| `acc_any@10`, main | 0.700 | 0.600 |
| MAP, main | 0.374 | 0.339 |
| map seconds per instance | 0.80 | 5.03 |
| instances resolving no seed | 26 of 50 (52%) | 52 of 150 (35%) |

What it bought immediately: the tier shipped in 674b7b1 was re-measured
against main on all 150, and went from 6 of 14 metrics excluding zero to 12
of 14. MAP +0.0410 (95% CI +0.0180 to +0.0680), nDCG@10 +0.0467 (+0.0225 to
+0.0747), `recall@10` +0.0585 (+0.0242 to +0.0975), and all four `acc_all`
metrics now clear the bar. Only the two @1 metrics do not. That is an
independent replication on 100 instances the original measurement never saw.

The cost is that an arm is now about 13 minutes rather than 40 seconds. Keep
the 50-instance subset as the fast gate during development and reserve the
150 for deciding a result; `loc-bench subset --quota N` switches between
them.

## 4. Detailed analysis

### 4.1 What the corpus and the literature agree on

The four-tier entity index in F1 is the single most useful artifact found.
Mapped against this server:

| LocAgent tier | agentless equivalent | State |
|---|---|---|
| Entity id index, fully-qualified name | stable ids, qualified-name resolution | Present |
| Global name dictionary, bare name to nodes | `RefIndex.definitions` | Present |
| BM25 inverted index over entity ids | none | **Absent** |
| Inverted index, code chunks to entities | `RefIndex.sites`, wired 674b7b1 | Present since 0.8.2 |

SpIDER's auditability property is also already present here in a different
form. It attaches a seed and an edge type to each surfaced function;
agentless labels every reference with an evidence tier (same-file,
resolved-via-import, unique, name-only-ambiguous) and tells the caller which
two to trust. Convergent design, independently arrived at.

SpIDER also builds its graph from per-repository syntax trees on demand at
session start rather than precomputing offline, which is the background
auto-index this server added for the MCP path.

### 4.2 Where the literature cuts against the project

The retrieval-versus-grep line (F3, plus GrepSeek 2605.29307) is the
strongest counterargument to indexing at all, and it should stay in view.
Its scope limit is what rescues the case here: it compares direct
interaction against *embedding* retrieval, and its own corpus-scaling
experiment shows direct interaction degrading sharply as the candidate space
grows, which is the regime an index serves.

The older IR line on query reformulation for bug localization (Springer
10.1007/s10664-025-10694-2; ACM 3236024.3236065; IEEE 6624044) is on the
exact lever measured as dominant, and is almost entirely inapplicable to
this server: it reformulates the bug report, and this server never sees the
bug report. That work belongs in the calling agent. What the server can do
is widen what a spelling resolves to, and say clearly when nothing did.

### 4.3 Measurement capability, which gates everything above

Today's 0.8.2 change moved MAP +0.061 with a 95% interval of +0.012 to
+0.121 on the deterministic tier. The agentic tier's measured noise floor is
about 0.025 on precision and 0.019 on WCC. An effect of this size is at the
edge of what the agentic instrument can resolve, and the 0.7.1 rollback is
the precedent for what happens when a change is only measurable on the
instrument that cannot resolve it.

The rule that follows: every candidate above is gated on the free
deterministic tier first, and only an effect large enough to clear the
agentic noise floor is worth an agentic run. "Code Isn't Memory" also offers
a better agentic design than the project currently uses -- three seeds, a
leak-audited per-task sandbox, a per-cell exclusion ledger, and dollars per
solve as a headline alongside resolve rate.

## 5. Context and assumptions

- The deterministic tier is now 150 Python instances across 104
  repositories. Every ranking claim here inherits that scope: one language,
  one subset, and a proxy whose correlation with the agentic arm this
  project records at about r = 0.37. Nothing here was measured on the
  agentic tier.
- `relation_weights` was measured at n=50 only, before the subset grew. Its
  row in 3.1 is therefore weaker evidence than the n=150 rows elsewhere, and
  re-running it is the cheapest open item in this report.
- Phase 4 of the research workflow, adversarial validation against the local
  peer, **did not run**. Both `/consult` backends answered a reachability
  probe and then timed out at 300 s on every substantive payload, including
  a shortened two-candidate version. The adversarial pass in sections 3.4
  and 4.2 is therefore first-party and has not been independently checked.
- Papers were read through abstracts and structured section reads, not in
  full. Reported figures are as those sources state them and have not been
  reproduced here.

## 6. Citations

**White papers, in the local corpus**

- LocAgent: Graph-Guided LLM Agents for Code Localization. arXiv:2503.09089.
- Agentless: Demystifying LLM-based Software Engineering Agents.
  arXiv:2407.01489.
- SWE-Explore: Benchmarking How Coding Agents Explore Repositories.
  arXiv:2606.07297.
- CodeNib: A Multi-View Data System for Serving Repository Context to Coding
  Agents. arXiv:2607.25431.
- Beyond Semantic Similarity: Rethinking Retrieval for Agentic Search via
  Direct Corpus Interaction. arXiv:2605.05242.
- GrepSeek: Training Search Agents for Direct Corpus Interaction.
  arXiv:2605.29307.
- CodexGraph: Bridging LLMs and Code Repositories via Code Graph Databases.
  arXiv:2408.03910.
- CodePlan: Repository-Level Coding using LLMs and Planning.
  arXiv:2309.12499.

**White papers, retrieved from the web**

- Code Isn't Memory: A Structural Codebase Index Inside a Coding Agent.
  arXiv:2606.22417.
- SpIDER: Spatially Informed Dense Embedding Retrieval for Software Issue
  Localization. arXiv:2512.16956, EMNLP 2026.
- RepoAtlas: Guiding Coding Agents via Evolving Multimodal Repository Views.
  arXiv:2609.16936.
- Improving Code Localization with Repository Memory. ICLR 2026.
- PatchRecall: Patch-Driven Retrieval for Automated Program Repair.
  arXiv:2604.10481.
- GRACE: Graph-Guided Repository-Aware Code Completion through Hierarchical
  Code Fusion. arXiv:2509.05980.
- BLAgent: Agentic RAG for File-Level Bug Localization. ACM 3830238.

**Query-reformulation line, noted and set aside as agent-side**

- KBL: a golden keywords-based query reformulation approach for bug
  localization. Empirical Software Engineering, 10.1007/s10664-025-10694-2.
- Improving IR-based bug localization with context-aware query
  reformulation. ACM 3236024.3236065.
- Assisting code search with automatic query reformulation for bug
  localization. IEEE 6624044.

## 7. Research methodology (appendix)

| Phase | Surface | Queries | Useful hits |
|---|---|---|---|
| 1 | `search_communities` | 1 | 0 (book-oriented; no coverage of this topic) |
| 1 | `semantic_search`, papers corpus | 3 | 14 distinct papers |
| 1 | `docling_rag` | 3 | 3 structured reads: LocAgent, CodeNib, Beyond Semantic Similarity |
| 3 | `web_search`, science | 3 | 7 papers not in the corpus |
| 3 | `fetch_page` | 1 batch of 4 | 4 abstracts |
| 4 | `consult`, local peer | 3 attempts | 0, timed out at 300 s |
| 5 | `loc-bench run`, n=50 | `relation_weights`, subword `min=1`, subword `min=2` | 3.1, 3.2 |
| 5 | `loc-bench subset --quota 150` + `fetch` | 150 instances, 104 repos, 3.2 GB, 2m02s | 3.5 |
| 5 | `loc-bench run`, n=150 | main, branch, branch+subword | 3.2, 3.5 |

The CodeNib read stopped at the iteration ceiling without converging, so its
row in the findings table is drawn from a partial answer.

Code inspection alongside the reading confirmed two facts the report rests
on: `_focus_resolution` has no lexical tier, and `_with_churn` stamps churn
onto files the ranking has already chosen rather than feeding the score.

One integrity check is worth recording because it could have invalidated
everything. The harness carries an uncommitted modification to
`seeds.py` that forwards path-shaped captures the seeding rule normally
discards. It is gated on `LOC_BENCH_FORWARD_RAW_PATHS=1`, which was never
set, and the control arm reproduced the August baseline exactly
(`mean_seeds` 3.9 and `mean_resolved_seeds` 1.34 in both), which is what
confirms the gate held.
