# Research Report: improving structural, model-free code localization

**Date**: 2026-09-27 | **Query**: techniques to improve seed resolution,
graph ranking, retrieval fusion and context budgeting for the agentless-mcp
ranking core | **Tree**: `feat/0.8.2-spelling-seeds` at 674b7b1

## 1. Executive summary

The literature converges on the design this server already has, and the one
gap it names is the one measured as dominant today: turning a caller's words
into graph entry points. LocAgent runs a four-tier entity index; 0.8.2 just
shipped its fourth tier, and the third -- a lexical index for keywords that
miss exact names -- is the only tier still absent.

**Confidence**: High on the gap analysis, which is a direct structural
comparison against a published index design. Medium on the ranked
recommendations, because two of the four cited results transfer from systems
with an LLM in the retrieval loop, which this server does not have.

**Primary sources**: LocAgent (2503.09089); Code Isn't Memory (2606.22417);
Beyond Semantic Similarity (2605.05242).

**Top two recommendations**, priority order:

1. Prototype a subword lexical tier and measure it on precision metrics
   first, not recall. The upside is the largest available; so is the risk.
2. Segment the benchmark by gold-set size. The literature says structural
   ranking pays off specifically on multi-file changes, and 0.8.2's largest
   and most significant gains were on `acc_all`.

**Measured during this pass, and not recommended**: turning on
`relation_weights`. Section 3.1 has the numbers. The direction is favourable
and no metric is harmed, but one marginal significance in fourteen tests is
what fourteen tests do, and MAP is flat at +0.0013.

**Recommended against on design grounds**: folding commit churn into the
rank, and making the map exploration-aware. Section 3.4 gives the reasons.

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
requires every gold file in the top k, and the subset averages 2.42 gold
files per instance.

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

### 3.2 The subword lexical tier (F1)

The gap is real and the mechanism is clear. `_focus_resolution` in
`application/map_service.py` now ends with an exact-occurrence tier; a
subword tier would sit after it, splitting both the query term and the
indexed identifiers on `snake_case` and `camelCase` boundaries.

**Measure precision before recall.** Today's exact tier was safe because its
matches are rare: the seat-cap sweep moved MAP only from +0.058 to +0.071
across caps of 2 to unbounded, which is what a rare, high-precision match
looks like. Subword matches will not be rare. A query term `queue` would
touch every queue in the repository, and the harness's own seeding rule
emits prose such as `GitHub` and `PostgreSQL`, which subword splitting would
make resolvable to something. That is the confident-seed-on-an-unrelated-file
defect the step-3 stop in `focus_paths` exists to prevent.

Acceptance should therefore be `acc_any@1` and MAP holding or rising, not
`recall@10` alone.

### 3.3 Benchmark segmentation (F2)

Split every future loc-bench report by gold-set size: single-gold instances
against multi-gold. The prediction from F2 is that structural ranking's edge
concentrates in the multi-gold half. 0.8.2's own result is consistent with
it: `acc_all@3` and `acc_all@10` both moved +0.080 with intervals excluding
zero, the largest gains in the table.

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

- The deterministic tier is 50 Python instances. Every ranking claim here
  inherits that scope: one language, one subset, and a proxy whose
  correlation with the agentic arm this project records at about r = 0.37.
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
| 5 | `loc-bench run` | 1 arm, `relation_weights` on | recorded in 3.1 |

The CodeNib read stopped at the iteration ceiling without converging, so its
row in the findings table is drawn from a partial answer.

Code inspection alongside the reading confirmed two facts the report rests
on: `_focus_resolution` has no lexical tier, and `_with_churn` stamps churn
onto files the ranking has already chosen rather than feeding the score.
