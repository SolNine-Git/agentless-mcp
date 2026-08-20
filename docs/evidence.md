# Navigation evidence

The companion `swe-explore-bench` dataset contains 60 matched tasks in each of
three conditions at top 10:

| Condition | Available navigation |
|---|---|
| grep-only | Read, Glob, and Grep |
| free-choice | Read, Glob, Grep, and the 11 agentless MCP tools |
| tool-only | Read and the 11 agentless MCP tools; Glob and Grep withheld |

The table below was recomputed on 2026-08-19 from the 60 shared instance ids in
each condition's `results_pilot/<condition>/top10.jsonl`. F1 is the benchmark's
line-level localization F1. The interval is the interquartile range across
tasks, not uncertainty around the mean.

| Condition | n | mean F1 | median | standard deviation | IQR |
|---|---:|---:|---:|---:|---:|
| grep-only | 60 | 0.207 | 0.186 | 0.144 | 0.093–0.298 |
| free-choice | 60 | 0.256 | 0.238 | 0.201 | 0.107–0.328 |
| tool-only | 60 | 0.252 | 0.210 | 0.192 | 0.105–0.331 |

Paired mean F1 deltas, with deterministic 20,000-resample percentile bootstrap
intervals over the 60 task-level deltas:

| Comparison | mean delta | median delta | 95% bootstrap interval | wins / losses / ties |
|---|---:|---:|---:|---:|
| free-choice − grep-only | +0.049 | +0.015 | +0.007 to +0.096 | 32 / 18 / 10 |
| tool-only − grep-only | +0.045 | +0.015 | +0.011 to +0.083 | 33 / 17 / 10 |
| free-choice − tool-only | +0.004 | −0.002 | −0.035 to +0.046 | 24 / 21 / 15 |

A win or loss here means an absolute task-level delta greater than 0.01. The
free-choice mean is directionally above both constrained conditions, but it is
effectively tied with tool-only and their paired interval spans zero. The data
supports the value of making structural navigation available alongside literal
search. It does **not** establish that individual tool selections were correct,
that agents under-select the structural tools, or that the free-choice policy
itself adds measurable value over the tool-only condition.

## Raw structural-tool selection

The MCP proof logs contain the structural `tools/call` events. They do not
contain native Read, Glob, or Grep events, so literal-tool selection counts
cannot be reconstructed and are not estimated.

| Condition | runs | runs with a structural call | zero-call runs | calls | mean / median calls |
|---|---:|---:|---:|---:|---:|
| grep-only | 60 | 0 (tools unavailable) | 60 | 0 | 0 / 0 |
| free-choice | 60 | 39 | 21 | 136 | 2.27 / 1.5 |
| tool-only | 60 | 55 | 5 | 285 | 4.75 / 4 |

| Tool | free-choice | tool-only | total |
|---|---:|---:|---:|
| `find_symbol` | 26 | 104 | 130 |
| `expand_symbols` | 48 | 77 | 125 |
| `repo_map` | 41 | 56 | 97 |
| `find_referencing_symbols` | 8 | 23 | 31 |
| `get_symbols_overview` | 13 | 16 | 29 |
| `read_slice` | 0 | 6 | 6 |
| `list_dir` | 0 | 3 | 3 |
| all other MCP tools | 0 | 0 | 0 |

These are selection frequencies, not selection-quality labels. Conditioning
outcomes on whether the free-choice agent selected a tool is post-treatment
analysis and cannot establish that making the opposite choice would have
improved that task.

## Cost denominator and adjacent benchmarks

Complete model-usage records exist for all 60 grep-only and tool-only runs and
59 free-choice runs; one free-choice run timed out at 900 seconds without token
accounting. Counting input, output, cache-creation, and prompt-cache-read tokens,
mean end-to-end model usage was 376,228 tokens for grep-only, 512,553 for
free-choice, and 485,856 for tool-only. That is a 36% free-choice premium and a
29% tool-only premium over grep-only. Prompt-cache reads dominate all three
figures, so they are model-session accounting, not just bytes returned by read
tools.

The often-cited 235-token orientation result is from the third-party
[`graphify-mcp`](https://github.com/yasinyaman/graphify-mcp) project, not from
Graphify's own benchmark. It compares one already-built `graphify_locate`
orientation response with a reported 61,836-token grep-and-read exploration on
six queries over httpx. It excludes graph construction and the targeted code
hydration that follows orientation. The experiment above measures complete
per-task localization sessions across 60 SWE tasks, including retrieval and
agent turns. The denominators differ, so the absolute token figures neither
confirm nor contradict one another.

## Downstream experiment still required

Localization is not resolution. The decisive follow-up holds the model and
task set fixed, varies only the three navigation conditions, and measures test-
verified resolve rate plus total model tokens through resolution, including
retries. `validate` supplies the pass/fail oracle and `vote` can arbitrate
equivalent independently sampled patches. Report both resolve rate and cost per
resolved task; do not convert a localization F1 advantage into a patch-quality
claim before that experiment runs.
