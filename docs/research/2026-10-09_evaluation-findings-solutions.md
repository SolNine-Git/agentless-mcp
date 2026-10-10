# Research Report: solutions for the 2026-10-09 functional evaluation

**Date**: 2026-10-09 | **Tree**: `fix/evaluation-findings` at 72d44d7 (0.9.0) |
**Query**: validate the best solution for each of the fourteen problems in
the external functional evaluation, against books, papers, specifications
and production tools.

## 1. Executive summary

The literature supports the proposed fix, or a refined form of it, for
twelve of the fourteen problems. For the other two (structured results,
lexical search) a better-supported alternative exists. No source argues
against fixing the five reproduced accuracy gaps. Where the sources
disagree with the evaluation, they narrow the fix rather than reject it.

**Confidence**: High on binding resolution, history, worktree pinning, the
tokenizer, single parsing and the test formats: primary specifications and
source code agree, and git behavior was reproduced by experiment. Medium on
map pinning, cycles and parse diagnostics: production practice agrees, but
no study measures the variant. Low on lexical search and response format:
no study measures the case this server is in.

**Primary sources**: ECMA-262 ResolveExport and Pyright
`resolveAliasDeclaration` (aliases); Creager and van Antwerpen, stack
graphs, arXiv 2211.01224 (per-reference resolution); git-log documentation
plus experiment (history); MCP specification 2025-06-18 through 2026-07-28
(structured content and pagination).

**Top recommendations, in order**:

1. Bind each alias to (defining module, original name), follow re-exports
   with a visited set, and give each reference occurrence its own tier with
   a per-row label for languages that have no scope analysis.
2. Resolve history spans in the HEAD blob at the full sha. The current
   behavior returns a silently incomplete commit list, not only a warning.
3. Pin exact focus symbols ahead of the score-ordered packing, with a cap
   on pins and a location stub for a pin that cannot fit.

**Gate for every change**: no degradation on the benchmark method used so
far (section 3.1). The gate is non-degradation, not proof of improvement.

**Corrections this pass makes to the first review** (the review given in
conversation before the research):

- Single parsing saves about 20% of an uncached call, measured unprofiled,
  not the 12% the evaluation derived from a profiled run. It moves from
  "defer" to "do after the correctness fixes".
- The history defect is worse than a disclosed limitation. A reproduced run
  returned a commit list that silently omitted a commit that changed the
  function.
- The captured sha is an 8-character prefix (`rev-parse --short=8`). Both
  the history fix and the worktree pin must capture the full sha.

**New defects found during the pass** (each verified in code at 72d44d7):

- `CachedSource._fresh_file_id` computes `content_digest(text)` once per
  fact reader, so every file is hashed three times per call
  (`core/cache.py` lines 496-521, 591-608).
- `TreeSitterExtractor.get_parser` says "Return the memoized parser"
  (`core/extractor.py` line 1914). `grammars.get_parser` returns a fresh
  parser by design (`core/grammars.py` lines 485-503).
- `githistory._SHA_PATTERN` is `[0-9a-f]{40}` (`core/githistory.py` line
  26), so the history tool rejects every commit in a SHA-256 repository.
- `docs/plans/2026-08-28_query-shaped-loading.md` predates the data-file
  exclusion (`in_name_graph`, 5639166) and its `refs(name)` index does not
  cover the role filter that `files_referencing` applies.

## 2. Key findings

| # | Problem | Verdict on the proposed fix | Refinement the sources add | Confidence |
|---|---|---|---|---|
| 1 | Per-file evidence tiers | Acceptable | Label each row whose language has no scope analysis; a `capabilities` line alone is not read | High (design), Low (labeling) |
| 2 | Aliases and re-exports | Best | Visited set is the correctness mechanism, depth bound only a guard; add an inverse alias index for fan-in | High |
| 3 | Exact focus dropped from map | Best | Cap pins; pinned rows pay first; stub `path:line` row when a pin cannot fit | Medium |
| 4 | Parse quality invisible | Best | Per-file flag plus error count and first error line; gate hints on relevance; measure C/C++ noise first | High (semantics), Medium (noise) |
| 5 | Cycles omit members | Acceptable | Add a bounded, weighted cut set; shortest witness by breadth-first search | High (components), Medium (witness) |
| 6 | Every call loads every fact | Best under the per-call contract | Five plan fixes (section 3.6) | Medium-High |
| 7 | Three parses per uncached file | Best | Gain is about 20% of an uncached call; Query API is not faster than a cursor walk | High (direction), Medium (size) |
| 8 | Descriptions disagree with behavior | Acceptable | Also mark the pick-one result at the top of the answer | Medium |
| 9 | Text-only results, no continuation | Better alternative exists | Structured content only for clients on a newer protocol; cursor as a tool argument bound to generation and query | High (spec), Low (format effect) |
| 10 | Tokenizer rejects special tokens | Acceptable | Use `encode_ordinary` | High |
| 11 | History reads working-tree lines | Best (option a) | Full sha, identity check beyond the ordinal, report shallow repositories | High |
| 12 | Worktrees follow a moving HEAD | Best | Full sha | High |
| 13 | No lexical search for MCP-only clients | Acceptable in changed form | An operation on `read`, never a ranking seed; refuse oversized result sets | Low |
| 14 | Benchmark not reproducible in-repo | Best (fixtures in tests/) | Negative assertions; cluster errors by repository | High (formats) |

## 3. Implementation guide

### 3.1 The gate: no degradation on the established benchmark

Every change on this branch must leave the benchmark method used so far no
worse. A measured improvement is welcome but not required. Most fixes here
correct answers the benchmark never exercises (cycles, history, worktrees,
parse diagnostics). Such a fix is acceptable when a test proves the
correction and the benchmark shows no degradation.

**Which instrument sees which change.** The bench session read both
harnesses and this tree at 72d44d7. Two facts decide most rows. The
retrieval tier runs `map --granularity file` and scores only the ranked
file list, so it cannot see symbol packing inside a file. The map ranking
is `graph.build_graph` (file-level edges into personalized PageRank), and
`core/graph.py` never imports `core/resolve.py`. A change confined to the
resolved graph is therefore invisible to the retrieval tier.

| Change | Instrument that can see it | Blind instruments |
|---|---|---|
| Exact focus pin | Map characterization goldens (function granularity); agentic bench | Retrieval tier |
| Per-occurrence tiers | A refs characterization golden (none exists yet; add one); agentic bench, underpowered | Retrieval tier |
| Aliases, re-exports in `resolve.py` | Fixtures; explain and path goldens | Retrieval tier |
| Aliases in `graph.py` import resolution | Retrieval tier | None |
| Parse diagnostics | Unit tests; a negative-answer golden (add one) | Both benches |
| Cycles | The three cycles goldens (repo_py, repo_ts, repo_nested): their diff is the primary evidence | Both benches |
| History | Unit test against a fixture repository (agents called history in 3 to 6 of 60 instances) | Both benches |
| Worktree pin | Unit test | Both benches |
| Descriptions | `test_the_eager_schemas_stay_under_their_context_ceiling` for size, when the edited description belongs to an always-loaded schema; wording effect unmeasurable in practice | Retrieval tier |
| Tokenizer | Unit test; no golden moves, because ids change only for text that holds special tokens and the default counter is chars/4 | Both benches |
| Single parse, query-shaped loading | Retrieval tier `map_seconds` (about 755 s at n=150 on main), with the ranking metrics held fixed as the correctness check | None |

**Decision rule** (the bench session's established rule):

- Paired bootstrap over instances, 20,000 resamples, 95% percentile
  interval, on the same instance set in both arms.
- Retrieval tier: report all fourteen metrics. MAP and nDCG@10 carry
  precision and recall@10 carries coverage. Agentic bench: report all
  twelve; weighted_core_coverage and hit_region_rate carry the answer.
- No fixed count of clear intervals. The rule is sign agreement across
  metrics that measure the same thing: 12 of 14 clear and favorable
  shipped the spelling tier, while 1 of 14 rejected relation_weights.
- When an interval spans zero, the instance up and down counts outrank the
  mean. The rejected subword tier had a positive mean MAP built from 12
  instances up and 16 down.
- Agentic noise floor: about 0.025 on precision and 0.019 on WCC, from the
  0.8.0 treatment-against-treatment pair. A delta inside it is not
  interpretable. Run replicates when a delta is within about twice the
  floor, when worker count or machine load changed between arms, or before
  publishing a result. The retrieval tier is deterministic (0 of 150
  instances differ between runs), so spend its replicate on a byte-diff.

**Procedure:**

1. Never point a harness at the shared working tree: an edit during a run
   splits the arm across two builds (integrity incident 3 in
   `docs/analysis/benchmark-methodology.md`). Create pinned worktrees at
   72d44d7 and at the branch sha, and `uv sync --all-extras` in each.
2. Run the retrieval tier on the 72d44d7 worktree first and byte-diff
   `ranked_files` against `results/i56-base.jsonl`. A match gives the
   baseline and the determinism control in one 12-minute run. The harness
   records no agentless commit, so this run is what proves provenance.
3. For an agentic arm: run the preflight probe (it must list 6 tools), set
   `AGENTLESS_PROJECT` to the worktree, run detached and solo with one
   worker, verify every proof log's `server_argv_sha256`, and report the
   non-bulk cohorts beside the headline. The control for "do the fixes help
   the agent" is the hooked arm at 72d44d7 (about $18); the 2026-09-27
   `claude_code` baseline can be reused.
4. Leave `LOC_BENCH_FORWARD_RAW_PATHS` unset: the harness's `seeds.py`
   carries an uncommitted instrument behind it.

**Acceptance criteria, fixed before any run.** The 0.7.1 release built two
evidence tiers and a denser fan-in, could not attribute a difference across
eight arms, and withdrew the work. Changes 1 and 2 of section 3.10 sit in
the same territory, so their criteria are written here first:

- Every change: its correctness test passes, and every golden diff is
  reviewed and explained line by line.
- Any change that touches `graph.py`, the map packing or the token budget:
  the retrieval tier shows no metric clear of zero in the negative
  direction, and down counts do not exceed up counts on MAP.
- The focus pin and the fan-in tiers ship on correctness evidence alone
  (fixtures and goldens). An agentic arm is optional. If one runs, a
  negative WCC or hit_region_rate delta outside the noise floor withdraws
  the change.

Two methodology points from the research apply to the instrument itself,
not to this branch. Single-run pass@1 on agentic evals varies by 2.2 to
6.0 points between runs, with a standard deviation above 1.5 points at
temperature 0 (On Randomness in Agentic Evals, arXiv 2602.07150). The 150
retrieval instances come from 104 repositories, so clustered standard
errors (Miller, arXiv 2411.00640) may be the more honest interval. Both
belong to the bench session to decide.

### 3.2 Problem 1: per-occurrence evidence

Resolve each occurrence before grouping, through the same per-site rule the
resolved graph uses, so one resolution rule serves both views. Every
production index stores a symbol and a role per occurrence (SCIP
`SymbolRole`, Kythe anchors), and scope graphs and stack graphs resolve each
reference to its own declaration.

Two refinements:

- **Label the row, not only the language list.** A row from a language with
  no scope analysis carries a label that says so. Sourcegraph separates
  "precise" from "search-based" results in the same panel for the same
  reason.
- **TypeScript stays wrong until it has a scope source.** Per-site tiers fix
  Python only. A vendored `locals.scm` could later demote rows that bind to
  a local or parameter, but upstream coverage is thin (no `locals.scm` in
  the python, go or rust grammar repositories) and the nvim queries contain
  visible errors. Use it to demote, never to promote.

The output format changes: one file can now hold rows at several tiers.

### 3.3 Problem 2: aliases and re-exports

Bind each local spelling to the pair (defining module, original member
name). Follow re-export chains with a visited set keyed on (module, name):
ECMA-262 `ResolveExport` uses `resolveSet` and Pyright uses
`alreadyVisited`. Keep a depth bound only as a resource guard. The fan-in
listing also needs an inverse alias index, so a query for `lib.helper`
collects the `h()` sites.

Pitfalls the sources name:

| Case | What the sources do |
|---|---|
| `__all__` | Pyright accepts fixed idioms only (`= [...]`, `+=`, `.extend`, `.append`, `.remove`); CodeQL treats any other form as exporting everything |
| Star re-exports | Two star exports of the same name re-export neither (MDN); `export *` skips `default` |
| `from pkg import x` | `x` may be a symbol in `__init__.py` or a submodule; Pyright falls back to the submodule |
| Relative re-export | `from .foo import bar` in a package also binds `foo` (CodeQL) |
| Conditional imports | Pyright deprioritizes declarations inside `except` blocks |
| Dynamic exports | Keep the weaker tier; CodeQL documents its own re-export rule as "a bit simplistic" |

### 3.4 Problem 3: exact focus in the function map

Seat the symbols the focus names first, then fill the remaining budget by
score. `MapResult.focus_order` already carries the ids, and putting them at
the front of `eligible` keeps the packing a prefix.

- Pinned rows pay their token cost first, and the score-ordered packing
  fills only the budget that remains.
- Cap the number of pins so a broad focus cannot spend the whole budget.
- When a pin cannot fit alone, emit a `path:line` stub row. Moatless and
  SWE-agent both report an oversized result instead of dropping it.
- Keep the pinned row in its file and line order, and name the pin in the
  map header. No source measures position inside a 500-token tool result.

aider does not solve this case. Its `mentioned_idents` boost applies only to
names that something references, so a name with no references gets no
boost. This is an inference from `aider/repomap.py`, not a run.

### 3.5 Problem 4: parse diagnostics

Keep the recovered facts. Store a per-file error flag, an error count and
the first error line, which `normalize.py` already computes
(`_error_count`, `_first_error_line`). Move those helpers to a shared
module rather than importing them from `normalize`. GitHub's tagger returns
`(tags, has_error)` per file, and Semgrep records partial parses with
locations.

- **Gate the hint on relevance.** On a symbol miss, report a hint only when
  the query name occurs inside an error region; otherwise give one coverage
  line ("3 of 412 files in scope parsed with errors"). This design is a
  synthesis, not a sourced practice.
- **Measure noise first.** ERROR nodes on valid code are documented for C
  and C++ macros (tree-sitter-c #108, tree-sitter-cpp #85) and for syntax
  newer than the grammar (tree-sitter-python #346). The grammar repositories
  publish known-failure lists for python, go and rust but none for C or C++.
- **The flag is one-sided.** tree-sitter-python #170 shows invalid code that
  parses with no error, so a clean parse does not prove valid code.
- The cache already keys rows on the grammar version, which a recovered
  tree depends on.

LLM callers do not infer missing coverage on their own. Strong models
"often output incorrect answers instead of abstaining when the context is
not" sufficient (Joren, Sufficient Context, arXiv 2411.06037), and
attention "cannot easily attend to 'gaps'" (Fu, AbsenceBench, arXiv
2506.11440).

### 3.6 Problems 5, 6 and 7

**Cycles.** Report each component with its member count, a bounded member
list, and one witness. Martin's component-cycle chapter (Clean
Architecture, ch. 14) treats the cycle's members as having "in effect,
become one large component", so the component is the unit to report.

- Use a breadth-first shortest witness from the component's smallest
  member instead of the depth-first walk. This is reasoning, not
  measurement.
- Optionally add a bounded, weighted cut set, labeled "one cut set, not the
  only one", the way import-linter and grimp do (Eades-Lin-Smyth heuristic,
  weights from import counts). Count the weights from
  `resolved.import_edges()` before the `set()` dedup at `resolve.py` line
  840.
- Pass `unresolved_internal_imports` and the skipped-file count, as the
  `import_cycles` docstring already prescribes.
- Do not enumerate elementary cycles. A minimum feedback arc set is
  NP-hard, and Johnson's algorithm grows with the number of cycles.

**Query-shaped loading.** The plan matches the production principle
("it's laziness — the ability to skip huge swaths of code altogether",
Kladov, rust-analyzer blog, 2020). No production system checks content on
every query; that is this server's contract, and the evidence does not
justify changing it (hashing costs about 2 ms, about 1% of a call). The
plan needs five fixes before implementation:

1. Index `(name, role)`, because `files_referencing` counts only
   `role == REFERENCE`.
2. Filter verified ids through a temp table. A bound `IN` list fails above
   32,766 parameters.
3. Drop data-language files, as `in_name_graph` now does.
4. Hash each file once per call (the triple-hash defect above).
5. Expect the name projection to dominate at very large scale. An FTS5
   trigram pre-filter can narrow it, with `_matches` still deciding.

Extrapolated, not measured: about 10 µs per file to read and hash means
about 1 s per call at 100,000 files before any SQL runs.

**Single parsing.** Parse once, derive symbols, imports and references from
one tree, then drop it. aider and the tree-sitter tags convention extract
several roles from one parse; Semgrep and ast-grep share one tree across
many rules.

- Measured on this repository: one parse of every file takes 0.15 s, and
  three parses take about 0.46 s of 1.31 s of extraction. Parsing once
  saves about 0.30 s, about 20% of a 1.5 s uncached `find-symbol`.
- py-tree-sitter caches `.children`, so the second and third walks get
  cache hits. That is a real gain in production. A benchmark must still
  re-parse on every iteration.
- Do not keep Node objects inside the facts, or each file's tree stays in
  memory.
- The Query API is not faster: `captures()` plus a sort took 4.09 ms against
  3.78 ms for a cursor walk on `extractor.py`. Choose it only for
  maintainability.
- Parser state is already per call. Correct the stale "memoized"
  docstring.

### 3.7 Problems 8, 9 and 10

**Descriptions.** Rewrite `explain_target` and `reference_limit` to match
the tested behavior, and pin the published behavior with transport-level
tests. Description accuracy changes tool selection: accuracy smells moved
selection by 8.8% (From Docs to Descriptions, arXiv 2602.18914, tool
selection only). Also start an ambiguous explain answer with a marker, for
example "ambiguous: N definitions; explained <id> by <rule>". Today the
"also defined at" lines come after the card and do not name the rule. No
source measures refuse-with-candidates against pick-one.

**Structured results and continuation.** The specification text, unchanged
from 2025-06-18 to 2026-07-28: "a tool that returns structured content
SHOULD also return the serialized JSON in a TextContent block". Cursor
pagination covers list operations only.

- Send `structuredContent` only to clients that negotiate a newer protocol
  version, or behind a flag, and keep the text byte-for-byte for everyone
  else. The GitHub MCP server does exactly this.
- Measured here: compact JSON costs 1.47x the text tokens for explain, 1.72x
  for refs and 1.08x for skeleton.
- Make a cursor a tool argument that carries the generation, a hash of the
  query and an offset. A stale or mismatched cursor returns a tool error
  with advice to rerun (Google AIP-158 rule). Pair it with "N more, refine
  with X": SWE-agent's iterative search scored 12.0 against 18.0 for
  summarized search, because agents paged through every match.
- Report the answer size in characters in the receipt. Claude Code moves
  any MCP result over 50,000 characters to a file.

**Tokenizer.** Use `encode_ordinary(text)`. The tiktoken source documents
it as "equivalent to `encode(text, disallowed_special=())` (but slightly
faster)". Both gave identical ids in the project venv; `allowed_special=
"all"` undercounted. Add a test that counts `<|endoftext|>` without error.

### 3.8 Problems 11 and 12

**History.** Experiment, git 2.55: with uncommitted lines above a function,
`git log -L` on the working-tree span returned only the creating commit and
silently omitted the commit that changed the function. With a larger
shift, git failed with `fatal: file a.py has only 11 lines`. The git-log
documentation states the rule: the range "must exist in the starting
revision", which "defaults to HEAD".

- Read the blob with `git cat-file blob <sha>:<path>`, parse it, find the
  same symbol, and pass the full sha to `git log`.
- Match on qualname and ordinal, then confirm with the signature line or a
  body hash. When the check fails, report ambiguity rather than guess.
- Report "no committed history" for a symbol new in the working tree, and
  "path not at HEAD" for an uncommitted rename.
- Check `git rev-parse --is-shallow-repository`: a depth-2 clone attributed
  a function to the shallow boundary commit.
- Committed renames already work: `-L` follows them with no flag.
- Reject `-L :funcname:`. CodeShovel (Grund, ICSE 2021) measured complete
  histories for 63% with the line-range mode and 41% with the funcname
  mode.

**Worktrees.** Experiment: worktrees added at `HEAD` before and after a
commit landed on two different commits, while the pinned worktree stayed
on the captured one. Pass the full sha (`git rev-parse --verify
HEAD^{commit}`) to every `worktree add`, and record it in the report. A
detached worktree HEAD protects its commit from gc (git `reachable.c`,
confirmed by experiment), and a lost commit fails loudly with
`fatal: invalid reference`.

### 3.9 Problems 13 and 14

**Lexical search.** No paper compares structural-only tools with
structural plus lexical tools: every measured arm adds structure to a
lexical baseline (Code Isn't Memory, arXiv 2606.22417). If an MCP-only
client becomes a target, add a bounded literal search as an operation on
`read`, not as a seventh tool. Return no hits and a refine message above N
results (the SWE-agent pattern). Never feed its results to the ranking: the
BM25 seed tier was rejected at 12 instances up and 16 down (2026-09-27
report, section 3.2). Tool-count costs are measured only at 30 tools and
above.

**In-repo benchmark.** Write the semantic corpus as targeted fixtures in
`tests/`, in the stack-graphs style: assertions in source comments, and an
empty assertion that a name resolves to nothing, which tests precision.
rust-analyzer (`$0` cursor, `//^^^` target) and Pyright fourslash
(`verifyFindAllReferences` over an exact set) use the same shape. The
existing characterization goldens already play the snapshot role. Keep
latency and memory out of pytest, because the hermetic-test rule forbids
real clocks.

### 3.10 Order of work

| Stage | Changes | Gate |
|---|---|---|
| 0 | Retrieval tier on a 72d44d7 worktree, byte-diffed against `results/i56-base.jsonl` | Byte-identical ranking |
| 1 | Tokenizer, descriptions and explain marker, full-sha capture, worktree pin, cycles (members, witness, counts), stale docstring, SHA-256 pattern | Unit tests; cycles golden diff reviewed; eager-schema ceiling test; retrieval-tier byte-diff unchanged |
| 2 | Exact focus pin | Fixtures; map golden diff reviewed; retrieval-tier byte-diff unchanged |
| 3 | Per-occurrence tiers with row labels | Fixtures; a new refs golden; agentic arm optional |
| 4 | Alias binding, then re-exports, in `resolve.py`; decide separately whether `graph.py` import resolution changes too | Fixtures with negative assertions; retrieval-tier non-degradation if `graph.py` changes |
| 5 | History in the HEAD blob; parse diagnostics after a noise measurement | Unit tests; a negative-answer golden |
| 6 | Single parsing, single hashing, then query-shaped loading with the plan fixes | Byte-identical output; retrieval tier `map_seconds` |
| Deferred | Structured content, cursors, lexical search | A client that needs them |

## 4. Detailed analysis

### 4.1 Authoritative sources

Name resolution is a settled problem in programming-language research and
in production indexers, and both resolve each reference on its own. Scope
graphs resolve "references in the scope graph ... to corresponding
declarations using a language-independent resolution process" (Neron et
al., ESOP 2015). Stack graphs keep that model at GitHub scale: "each name
binding in a program is represented by a path" (arXiv 2211.01224). The
per-file tier in this server is a shortcut that neither line of work takes.

Alias and re-export handling converges on one design across a language
specification and three implementations (ECMA-262, Pyright, stack graphs,
CodeQL). That agreement is the strongest evidence in this report.

The books contribute principle rather than mechanism. Clean Architecture
fixes the unit of a cycle report. DDIA's snapshot isolation section and
Continuous Delivery's "Only Build Your Binaries Once" both describe the
worktree defect: phases of one operation reading different states. DDIA's
column-oriented storage section and Database Internals support reading
only the columns a query uses.

### 4.2 Industry practice

Production tools treat bounded output the same way: report what did not
fit instead of dropping it (Moatless, SWE-agent, Sourcegraph, Serena). They
also label evidence classes in place (Sourcegraph precise and search-based
results) and flag partial parses per file (GitHub tags, Semgrep). The
GitHub MCP server is the one production precedent for structured content,
and it gates it on the negotiated protocol version.

### 4.3 Where the literature has no answer

- Refuse-with-candidates against pick-one for an ambiguous name.
- One multi-operation tool against separate tools at six tools.
- JSON against text for tool results fed back to a model. Tam 2024 ("Let
  Me Speak Freely?") tests the model's own output format, not tool results.
- A false-positive rate for ERROR nodes in tree-sitter-c or
  tree-sitter-cpp.
- Breadth-first against depth-first cycle witnesses.
- How code-navigation tools should phrase negative results for LLMs.
- Confidence labels or evidence tiers for LLM consumers of code graphs.
  ARISE (arXiv 2605.03117) argues that spurious call edges are "more harmful
  than missing edges" but cites rather than measures it.

### 4.4 Validation

Every code claim in sections 1 to 3 was checked against 72d44d7 by reading
the cited lines. The git claims rest on experiments run in throwaway
repositories, with commands and output recorded by the research workers.
The local-peer validation phase was waived by the user for this pass.

## 5. Context and assumptions

- The research workers searched arXiv through web search and read papers
  with page fetches. `search_arxiv` ran asynchronously and returned no
  inline results, and no paper was downloaded into the indexed library.
- Several author lists did not load (arXiv 2607.00725, 2602.18914,
  2605.03117). Those papers are cited by title and id only.
- The 4.6x packing figure (Recall Is Not Enough, arXiv 2607.00725) comes
  from RAG question answering, in a paper under review. It supports the
  principle of section 3.4 and nothing more.
- Timing figures come from one machine and one repository. The 100,000-file
  figure is an extrapolation.
- Section 3.1's instrument map, decision rule and procedure come from the
  bench session on 2026-10-09, read from both harnesses and this tree at
  72d44d7. The retrieval-tier baseline for 72d44d7 is a strong prior
  (`results/i56-base`), not a proven one, until stage 0 runs.

## 6. Citations

**Books**

- Martin, *Clean Architecture*, ch. 14, "The Effect of a Cycle in the
  Component Dependency Graph".
- Kleppmann, *Designing Data-Intensive Applications*, ch. 3
  "Column-Oriented Storage" and "Storing values within the index"; ch. 7
  "Snapshot isolation".
- Petrov, *Database Internals*, ch. 1 "Column-Oriented Data Layout".
- Humble and Farley, *Continuous Delivery*, "Only Build Your Binaries Once".
- Beyer et al., *Site Reliability Engineering*, "Hermetic Builds".
- Anderson, *Security Engineering*, 6.4.2 (race conditions).

**Papers**

- Neron, Tolmach, Visser, Wachsmuth, "A Theory of Name Resolution", ESOP
  2015.
- Creager and van Antwerpen, "Stack graphs: Name resolution at scale",
  arXiv 2211.01224.
- Liu et al., "Lost in the Middle", arXiv 2307.03172.
- Zhang et al., RepoCoder, arXiv 2303.12570.
- Chen et al., LocAgent, arXiv 2503.09089.
- Yang et al., SWE-agent, arXiv 2405.15793.
- Liu et al., CodexGraph, arXiv 2408.03910.
- Hsieh et al., "Tool Documentation Enables Zero-Shot Tool-Usage", arXiv
  2308.00675.
- Qu et al., DRAFT, arXiv 2410.08197.
- "From Docs to Descriptions: Smell-Aware Evaluation of MCP Server
  Descriptions", arXiv 2602.18914.
- "MCP Tool Descriptions Are Smelly!", arXiv 2602.14878.
- He et al., "Does Prompt Formatting Have Any Impact on LLM Performance?",
  arXiv 2411.10541.
- Tam et al., "Let Me Speak Freely?", arXiv 2408.02442.
- "Same Bytes, Different Authority", arXiv 2609.35932.
- Joren et al., "Sufficient Context", arXiv 2411.06037.
- Fu et al., AbsenceBench, arXiv 2506.11440.
- Asaduzzaman et al., LHDiff, ICPC 2013.
- Grund et al., CodeShovel, ICSE 2021.
- CodeTracker 2.0, arXiv 2409.16185.
- "Code Isn't Memory", arXiv 2606.22417.
- Paipuru, CodeCompass, arXiv 2602.20048.
- RAG-MCP, arXiv 2505.03275.
- "How Many Tools Should an LLM Agent See?", arXiv 2605.24660.
- "Recall Is Not Enough", arXiv 2607.00725.
- "On Randomness in Agentic Evals", arXiv 2602.07150.
- Miller, "Adding Error Bars to Evals", arXiv 2411.00640.
- ARISE, arXiv 2605.03117.

**Specifications, documentation and source**

- ECMA-262, ResolveExport.
- MCP specification, 2025-06-18, 2025-11-25 and 2026-07-28 (tools,
  pagination, stateful tools).
- Google AIP-158, pagination.
- git documentation: git-log (`-L`), git-blame (`--contents`), git-gc,
  git-worktree, git-rev-parse; git source `reachable.c`.
- tree-sitter documentation (code navigation, syntax highlighting), source
  (`parser.c`, `error_costs.h`, `crates/tags`), and grammar issues
  tree-sitter-c #108, tree-sitter-cpp #85, tree-sitter-python #170 and
  #346.
- Pyright `declarationUtils.ts`; stack-graphs Python TSG and test format;
  CodeQL `ImportResolution.qll`; SCIP `scip.proto` and CLI; Kythe schema;
  rust-analyzer `goto_definition.rs`.
- aider `repomap.py`; Moatless `search_base.py`; Serena `symbol_tools.py`;
  GitHub MCP server README; Sourcegraph code navigation and MCP docs;
  Semgrep output schema and language maturity levels; ast-grep
  `combined.rs`; import-linter `acyclic_siblings`; grimp usage; NDepend
  rules ND1400 and ND1401; tiktoken `core.py`; SQLite query planner and
  limits; Glean incremental blog; Kladov, "Three Architectures for a
  Responsive IDE"; Lippert, "Red-Green Trees"; Anthropic, "Writing
  effective tools for agents".

**Worth downloading later** (through `/crawl`): 2211.01224, 2605.03117,
2103.00587 (PyCG), 2405.15793, 2411.06037, 2506.11440, 2602.18914,
2602.14878, 2607.00725, 2602.07150, 2411.00640, 2409.16185, and the
CodeShovel and LHDiff papers.

## 7. Research methodology (appendix)

Six research workers ran in parallel, one per problem group: binding
evidence (1, 2), map focus, lexical search and benchmark (3, 13, 14),
LLM tool interface (8, 9, 10), parse quality and cycles (4, 5), fact
loading and parsing (6, 7), and history and worktrees (11, 12). Each
searched the docs corpus first, then the web with the science category,
then primary sources by direct fetch.

| Group | Tool calls | Notes |
|---|---|---|
| Binding evidence | 62 | About 30 primary pages read; corpus searches returned no on-topic passages |
| Map, lexical, benchmark | 55 | 17 fetch batches |
| Tool interface | 70 | Token costs and tiktoken behavior measured in the project venv |
| Parse quality, cycles | 107 | GitHub issue and code searches for grammar errors; Clean Architecture read in the vault |
| Loading, parsing | 88 | Two timing scripts in the scratchpad (`parse_split.py`, `query_vs_walk.py`) |
| History, worktrees | 80 | Throwaway repositories under `/tmp/claude-1000/gitexp/` |

The corpus answered the book questions (Clean Architecture, DDIA, Database
Internals, Continuous Delivery) and almost none of the program-analysis
questions: name resolution, tree-sitter and MCP have little coverage in
it. Web search engines were rate-limited during the pass, so most primary
sources were fetched by known URL.

## 8. Implementation and validation (2026-10-09)

Every stage of section 3.10 was built on `fix/evaluation-findings`, except
the deferred row and the open half of query-shaped loading.

**Gate result.** The retrieval tier ran from frozen copies of the tree
(rsync, no `.git`), so edits during a run could not split an arm.

| Run | Tree | `ranked_files` differing from `fix-base` | `map_seconds` |
|---|---|---|---|
| `fix-base` | 72d44d7 | -- (byte-identical to the stored `i56-branch` arm) | 508.0 |
| `fix-s1` | stage 1 | 0 of 150 | 475.6 |
| `fix-s4` | stages 1-4 | 0 of 150 | 472.7 |
| `fix-s6` | stages 1-6 | 0 of 150 | 383.0 |
| `fix-review` | stages 1-6 and the review fixes | 0 of 150 | 373.8 |
| `fix-qualifier` | the qualifier and module-attribute fixes | 0 of 150 | 416.8 |

No ranking moved, so no metric moved: the gate holds at every stage. The
`map_seconds` totals are single runs (`fix-qualifier` ran beside the surface diff) and are not claimed as an effect.

**Correctness evidence for what the retrieval tier cannot see.**

- Fan-in tiers: the evaluation's shadowing and module-qualifier probes now
  list the parameter and the other module's call as name-only, and the real
  caller as resolved-via-import. A new refs golden per fixture pins the
  output; TypeScript groups carry `locals not checked`.
- Aliases and re-exports: on the MONAI checkout, 5,558 reference edges moved
  from unique to resolved-via-import with the same target, 249 new
  resolved-via-import edges appeared, and none was lost (final build, with
  the module-attribute walk). Sampled chains (`swin_unetr.py` ->
  `monai.networks.blocks` -> `unetr_block.py`; `load` and `Affine` through
  package re-exports) were checked against the source. On 97a8e7e, before
  the review fixes: aiohttp 494 edges moved to resolved-via-import; this
  repository and `Agentless` did not change.
- Focus pin: the evaluation's probe (180 functions, then `target_bug`,
  budget 500) now keeps `target_bug`. The map goldens did not move.
- History: lines inserted above a symbol no longer shift the span; the
  research experiment is a test.
- Cycles: the two cycles goldens lost a note that counted only
  standard-library imports.

**Cost of the correctness changes**, measured on the MONAI checkout over
pre-parsed facts, interleaved runs, nine and seven samples:

| Call | Before | After |
|---|---|---|
| fan-in on `MetaTensor` (707 sites) | 0.352 s | 0.372-0.377 s |
| whole resolved graph | 0.523-0.528 s | 0.546-0.549 s |

The first alias-scan version cost 0.19 s on the fan-in; walking the import
chain before calling `resolve` brought it to the figure above.

**Parse diagnostics: noise measured first.** Committed files with a
tree-sitter ERROR node, 30 sampled benchmark checkouts plus the local
repositories: Python 0 of 7,493, C 248 of 1,131 (21.9%), C++ 47 of 746
(6.3%), TypeScript 25 of 176 (14.2%), every other language 0. A blanket
per-file flag would be noise, so the hint fires only on a lookup miss whose
query is spelled inside an unparsed region, and needs no cache schema change.

**Performance**, this repository, CLI wall time, interleaved:

| Call | Before | After |
|---|---|---|
| `find-symbol scan_repo --no-cache` (median of 7) | 1.518 s | 1.207 s |
| `find-symbol scan_repo`, warm cache (median of 9) | 0.302 s | 0.180 s |

The warm figure is the symbol-only scan; the single digest alone moved it
within noise (0.303 s to 0.299 s), as section 3.6 predicted.

**Pull-request review.** A read-only review of the branch found nine
defects. Two were regressions from 0.9.0: fan-in demoted a call when the
same line spelled the name again as a member or a keyword, and it listed an
aliased import line twice. The other seven:

- History refused a dirty file under a subdirectory root.
- A star hop outranked an explicit re-export and carried private names.
- The hop bound counted pairs visited, not depth.
- An alias-spelled parameter was listed.
- A failed shallow check read as "not shallow".
- `start_line` changed meaning for a dirty file.
- One docstring ran past one line.

All nine are fixed with tests, and the `fix-review` run repeats the gate.

**Surface diff over the 150 Loc-Bench checkouts.** The retrieval tier reads
only the ranked file list, so every changed surface was run under 0.9.0 and
under the final build on the same 1,128 queries: the function map with the
issue's seeds as focus, `cycles`, `refs` and `explain` on up to two seed
names, and `refs` on three sampled symbols. Targets were picked under each
build and were byte-identical. No model is involved, and no exit code
changed.

| Surface | Answers changed | What changed |
|---|---|---|
| `refs` | 130 of 678 | 1,706 lines unique to resolved-via-import, 415 ambiguous to resolved-via-import; 15 of 15 sampled were correct |
| `explain` | 39 of 228 | 16 add only the tie notice; the rest move tiers or edges through alias resolution |
| `map` | 42 of 150 | Every one has a seed that names a symbol (the prediction held); no file was added or lost |
| `cycles` | 150 of 150 | The note counts internal imports only; member lists; no cycle count fell |

Lines that left a binding tier: 22 resolved-via-import to ambiguous (14 are
modin's `pandas.DataFrame`, the third-party class; 8 are aiohttp names
imported under `TYPE_CHECKING` and assigned `None` at module level) and 92
same-file to ambiguous (parameters that shadow the target, pytest fixtures
among them). Two answers over the token ceiling show 10 and 12 fewer rows,
because per-tier groups spend more of the ceiling on headers; both carry
`truncated`. Fifteen maps list one to nine fewer symbols because a pinned
focus symbol took their room.

The first run of this diff found a regression no fixture covered: an
imported class used as a qualifier (`OptionKey.from_string(...)`) fell from
resolved-via-import to ambiguous on 31 lines, because the extractor marks it
`MODULE_QUALIFIER`. Fan-in now tiers a qualifier by the binding of the name
it spells, and a module attribute follows the package's re-exports; the
table above is after that fix.

**Not built.** Structured content, cursors and lexical search (deferred by
design); references loaded by name (the open half of the plan, now updated
with its five fixes); an agentic arm (optional under section 3.1 and not run).
