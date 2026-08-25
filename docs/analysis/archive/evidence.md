> Archived 2026-08-24: superseded by
> [`docs/analysis/benchmark-methodology.md`](../benchmark-methodology.md).

# Navigation evidence

## Correction, 2026-08-24: every figure below is invalid

Read this section before any number in this document.

The run this document reports, and the 0.6.0 run that followed it, both
measured a server that could not see the repositories under test. The
SWE-Explore-Bench harness unpacks each instance's snapshot repository into a
gitignored directory inside the bench work tree, and until 0.6.1
`core.treewalk.walk_repo` took its file list from git whenever git answered
for the root. git answers for such a root and lists zero paths for it, so
every snapshot repository read as empty: no files to walk, no files to map,
nothing to rank. The defect and its fix are the 0.6.1 entry in `CHANGELOG.md`.

What that costs this document: the two agentless arms in every table below
measured the server answering about empty repositories. The grep-only and
deterministic arms are unaffected by the defect, so their columns stand on
their own, but every comparison against an agentless arm -- the results table,
the paired intervals, the tool-selection counts, the span statistics and the
cost figures -- says nothing about the tools as they behave on a repository
they can read. The same holds for the 0.6.0 benchmark run and its
"recovers test-file localization" claim.

Nothing here is retracted as a negative result either. An arm that saw no
files is not evidence that the tools underperform; it is not evidence of
anything. The figures below are kept as the record of what was run. Replace
them by re-running the benchmark against 0.6.1 or later, by the procedure in
"How to regenerate this document", and confirm before scoring that an
`orient(map)` call against an unpacked snapshot lists its files.

## What this document reports

- Document date: 2026-08-23.
- Run: the six-batch SWE-Explore-Bench pilot, snapshot
  `results_pilot/scores_after_batch_6.json`, generated 2026-08-23T19:12:11Z.
- Sample: 60 instances scored by every arm, 33 unique repositories, seed 42.
  The language mix is python 36, go 6, javascript 5, java 2, rust 2, ruby 2,
  c 2, php 2, typescript 2, cpp 1. The instances come from three source
  datasets: verified 31, pro 18, multilingual 11.
- Model: `sonnet`, driven through the Claude Code CLI with a 900 second
  per-instance timeout.
- Server under test: `agentless-mcp` 0.5.0, published on the v2 surface of five
  tools.
- Ranking depth: every figure below is top 10 unless the text says otherwise.

The two agentless arms were re-run on 2026-08-23. The grep-only baseline and
the two deterministic arms last ran on 2026-08-19 and were not re-run, because
neither depends on the MCP server. Read every baseline comparison with that
four-day gap in mind.

**The harness is not in this repository.** The whole A/B layer -- the explorers,
`run_ab.py`, `score_report.py`, `span_metrics.py`, the proof logs and every
result file -- is uncommitted local work in a sibling clone,
`swe-explore-bench`. Nothing here is reproducible from this repository alone.
Treat these numbers as recorded measurements, not as auditable evidence.

## The arms

| Condition | Arm directory | Navigation offered |
|---|---|---|
| grep-only | `claude_code` | Read, Glob, Grep |
| free-choice | `claude_code_agentless` | Read, Glob, Grep, and the five `mcp__agentless__*` tools |
| tool-only | `claude_code_agentless_forced` | Read and the five `mcp__agentless__*` tools; Glob and Grep withheld |
| deterministic | `agentless_deterministic` | no model: the CLI map, then expand per ranked id |
| deterministic, interleaved | `agentless_deterministic_interleave` | the same, round-robin across files |

The five tools are `orient`, `symbols`, `read`, `find_referencing_symbols` and
`capabilities`. An earlier version of this document reported the v1 surface of
eleven tools (`find_symbol`, `expand_symbols`, `repo_map`, `read_slice`,
`list_dir` and the rest). The server still publishes v1 for un-migrated
operators, but v2 is the default surface and the only surface this run
measured.

## Read `recall@500` and `wcc@500`, not `f1_score`

The benchmark's own runbook says so, and the reason is a scoring artifact rather
than a preference. `eval.py:_resolve_interval` expands `end == -1` to the whole
file, and the ground truth is what a reference trajectory read, so a core region
is frequently an entire file. An arm that emits `path:1--1` therefore collects
near-perfect precision and recall without localizing anything.

`span_metrics.py` charges for span width. It allocates the predicted regions, in
rank order, into a fixed total line budget, truncates the region that overflows
the budget, drops everything after it, and hands the result back to the
harness's own `evaluate_recall` and `evaluate_weighted_core_coverage`.
`recall@500` reproduces the harness's shipped `recall_at_500` exactly, which is
the check that the budgeting is faithful.

The artifact is present in this run. The two largest per-instance f1 deltas
against the baseline carry 43.1% of the tool-only arm's net f1 delta mass and
41.2% of the free-choice arm's. The single largest is `django__django-16100`,
where the tool-only arm scored f1 0.999 against the baseline's 0.066 after
emitting `django/contrib/admin/options.py:1--1`. The free-choice arm's largest
is `fmtlib__fmt-3272`, f1 0.812 against 0.073, after emitting
`include/fmt/format.h:1--1`. The runbook's "72% from two instances" figure
describes the earlier 30-instance sample; the concentration is smaller here and
the mechanism is the same.

## Results

Means over the 60 shared instances at top 10. The first two columns are the
headline.

| Arm | recall@500 | wcc@500 | recall | precision | f1_score | hit_file_rate | weighted_core_coverage | cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| grep-only | 0.1640 | 0.1954 | 0.1781 | 0.6252 | 0.2071 | 0.6437 | 0.2069 | $19.72 |
| free-choice | 0.1722 | 0.1826 | 0.1937 | 0.6831 | 0.2452 | 0.6130 | 0.1959 | $18.89 |
| tool-only | 0.1958 | 0.1738 | 0.2149 | 0.7112 | 0.2600 | 0.5854 | 0.1990 | $19.22 |
| deterministic | 0.0485 | 0.0378 | 0.0485 | 0.2529 | 0.0547 | 0.1200 | 0.0415 | $0 |
| deterministic, interleaved | 0.0559 | 0.0568 | 0.0713 | 0.1028 | 0.0519 | 0.3292 | 0.0701 | $0 |

The two headline metrics disagree, and the disagreement is the main finding.
On `recall@500` the order is tool-only, then free-choice, then grep-only. On
`wcc@500` the order reverses: grep-only leads both tooled arms. The arms also
trade precision against file coverage. Both tooled arms quote narrower and
cleaner spans than the baseline and reach fewer of the right files.

The two deterministic arms are far behind every model-driven arm on every
column. They establish that the map-then-expand recipe alone does not localize;
they do not measure the tools as an agent uses them.

## Paired comparisons

Deltas are paired over the same 60 instances. The interval is a 95 percent
percentile bootstrap over the 60 task-level deltas, 20,000 resamples with a
fixed seed; the bounds move by about 0.001 between seeds. A win or a loss is an
absolute task-level delta above 0.01, and `p` is a two-sided exact sign test
over the wins and losses. `score_report.py` prints the means only. The paired
figures were derived for this document and no committed script reproduces them.

| Comparison | metric | mean delta | 95% interval | w / l / t | p |
|---|---|---:|---:|---:|---:|
| free-choice − grep-only | recall@500 | +0.008 | −0.022 to +0.034 | 25 / 17 / 18 | 0.28 |
| free-choice − grep-only | wcc@500 | −0.013 | −0.047 to +0.021 | 21 / 22 / 17 | 1.00 |
| free-choice − grep-only | hit_file_rate | −0.031 | −0.083 to +0.020 | 10 / 16 / 34 | 0.33 |
| free-choice − grep-only | f1_score | +0.038 | +0.011 to +0.070 | 35 / 20 / 5 | 0.06 |
| tool-only − grep-only | recall@500 | +0.032 | +0.006 to +0.062 | 24 / 15 / 21 | 0.20 |
| tool-only − grep-only | wcc@500 | −0.022 | −0.054 to +0.007 | 19 / 19 / 22 | 1.00 |
| tool-only − grep-only | hit_file_rate | −0.058 | −0.108 to −0.009 | 7 / 18 / 35 | 0.04 |
| tool-only − grep-only | f1_score | +0.053 | +0.020 to +0.095 | 34 / 15 / 11 | 0.01 |
| tool-only − free-choice | recall@500 | +0.024 | −0.007 to +0.057 | 25 / 20 / 15 | 0.55 |
| tool-only − free-choice | wcc@500 | −0.009 | −0.043 to +0.022 | 23 / 18 / 19 | 0.53 |
| tool-only − free-choice | f1_score | +0.015 | −0.031 to +0.062 | 27 / 24 / 9 | 0.78 |

What the table supports:

- The tool-only arm's `recall@500` advantage over the baseline is positive and
  its bootstrap interval excludes zero, but the sign test does not reach
  significance at 24 wins to 15 losses. Call it a directional gain on one of the
  two headline metrics, not an established one.
- The free-choice arm is indistinguishable from the baseline on both headline
  metrics.
- Neither tooled arm improves `wcc@500`. Both point estimates are negative and
  both intervals span zero.
- The tool-only arm loses file coverage. The `hit_file_rate` delta of −0.058 is
  the one comparison in the table where the interval excludes zero and the sign
  test agrees.
- The two tooled arms are not distinguishable from each other on any metric
  here.

The f1 ordering flipped since the 2026-08-19 report, where free-choice led
tool-only. Tool-only now leads, 0.2600 against 0.2452. The paired interval for
that pair spans zero, so read the flip as a re-ordering of two tied arms rather
than as a change in rank.

## Structural tool selection

The MCP proof logs record every structural `tools/call`. They record the tool
name only, never the operation, so a per-operation breakdown cannot be
reconstructed. They contain no native Read, Glob or Grep events, so literal-tool
selection is not counted and is not estimated.

| Condition | runs | runs with a structural call | zero-call runs | calls | mean / median calls |
|---|---:|---:|---:|---:|---:|
| grep-only | 60 | 0 (tools unavailable) | 60 | 0 | 0 / 0 |
| free-choice | 60 | 56 | 4 | 199 | 3.32 / 3 |
| tool-only | 60 | 59 | 1 | 216 | 3.60 / 3 |

Instances that invoked each tool at least once, and the raw call count behind
each:

| Tool | free-choice instances | free-choice calls | tool-only instances | tool-only calls |
|---|---:|---:|---:|---:|
| `orient` | 40 / 60 | 49 | 49 / 60 | 62 |
| `symbols` | 34 / 60 | 87 | 24 / 60 | 58 |
| `capabilities` | 25 / 60 | 26 | 46 / 60 | 46 |
| `read` | 17 / 60 | 34 | 28 / 60 | 48 |
| `find_referencing_symbols` | 3 / 60 | 3 | 2 / 60 | 2 |

Two observations, both selection frequencies rather than selection-quality
labels. `find_referencing_symbols` is named in the prompt recipe of both tooled
arms and is invoked in 3 to 5 percent of instances. `capabilities` is invoked in
more instances than `symbols` under the tool-only condition: the model asks the
server what it can do, then acts on the answer.

Conditioning an outcome on whether an arm selected a tool is post-treatment
analysis. It cannot establish that the opposite choice would have improved that
instance.

## Span shape

The budgeted metrics exist because of these counts, so they belong beside them.

| Condition | mean predicted lines | median region width | whole-file emissions | `end == -1` spans | regions |
|---|---:|---:|---:|---:|---:|
| grep-only | 703.1 | 25.0 | 33 | 9 | 378 |
| free-choice | 657.9 | 28.0 | 34 | 11 | 388 |
| tool-only | 702.4 | 26.5 | 22 | 7 | 366 |

A whole-file emission is a region covering at least 90 percent of its file. In
28 of the 60 instances at least half the resolvable core ground-truth regions
are themselves whole-file reads, which is what makes a whole-file emission pay.

## Cost denominator and adjacent benchmarks

Complete model-usage records exist for all 60 runs in each of the three
model-driven arms. Counting input, output, cache-creation and prompt-cache-read
tokens, mean end-to-end model usage was 376,228 tokens for grep-only, 697,277
for free-choice and 756,610 for tool-only. That is an 85 percent free-choice
premium and a 101 percent tool-only premium over grep-only. Prompt-cache reads
dominate all three figures, so they are model-session accounting, not bytes
returned by read tools. Dollar cost does not follow token count, because cache
reads are billed at a lower rate: the tooled arms cost slightly less than the
baseline over the same 60 instances. Mean turns were 8.8, 12.8 and 12.3, and
mean wall time 41.9, 53.3 and 53.4 seconds.

The often-cited 235-token orientation result is from the third-party
[`graphify-mcp`](https://github.com/yasinyaman/graphify-mcp) project, not from
Graphify's own benchmark. It compares one already-built `graphify_locate`
orientation response with a reported 61,836-token grep-and-read exploration on
six queries over httpx. It excludes graph construction and the targeted code
hydration that follows orientation. The experiment above measures complete
per-task localization sessions across 60 tasks, including retrieval and agent
turns. The denominators differ, so the absolute token figures neither confirm
nor contradict one another.

## Downstream experiment still required

Localization is not resolution. The decisive follow-up holds the model and task
set fixed, varies only the navigation conditions, and measures test-verified
resolve rate plus total model tokens through resolution, including retries.
`validate` supplies the pass/fail oracle and `vote` can arbitrate equivalent
independently sampled patches. Report both resolve rate and cost per resolved
task. Do not convert a localization advantage on one metric into a patch-quality
claim before that experiment runs.

## How to regenerate this document

From the `swe-explore-bench` clone root:

```
uv run python score_report.py --results results_pilot --top-k 10
```

That prints every mean in the results table, the budgeted columns, the span
statistics and the per-instance rows. The tool-selection counts come from
`mcp_proof_pilot/<arm>/*.jsonl`, one file per instance. The paired intervals and
sign tests are not printed by any committed script.
