# Benchmark methodology and results

This document records how this project measures whether its tools help an
agent localize code, and what the current measurement says. It is the durable
record: the numbers move with each release, the procedure and the guardrails
below do not. Read it before you re-run the benchmark, extend it, or quote a
figure from it.

It replaces [`archive/evidence.md`](archive/evidence.md), which reported a run
whose every agentless figure was invalid. Why that happened, and what the
procedure now does about it, is the "Integrity incidents" section. That section
is not history for its own sake. Both incidents produced numbers that looked
ordinary, and each one is the reason for a check that now runs before scoring.

## The instrument

**SWE-Explore-Bench.** The harness is a sibling clone, `../swe-explore-bench`.
It is not distributed with this repository and nothing here reproduces it. Treat
every figure below as a recorded measurement rather than as auditable evidence,
and re-measure rather than cite from memory.

The pilot sample is 60 issue-localization instances drawn from more than 30
repositories and 8 languages. Each instance gives the agent an issue and asks
which files and line regions it must read to resolve it. The ground truth is
what a reference trajectory actually read: the `read_core_files` and
`read_core_regions` fields of `pilot_sample.jsonl`. That definition matters when
you read a score. A core region is frequently a whole file, because the
reference trajectory read a whole file.

**The metrics.** The harness's `ExploreEvaluator` computes 17 of them:
line-level precision, recall and F1; hit and noise rates at file level and at
region level; weighted core coverage (WCC); nDCG at the budget; recall at the
budget; first-useful-hit; and context efficiency. No single one of them is the
score. WCC and the budgeted recall carry the localization question; the
line-level three are sensitive to span width; the hit rates say whether the
right file was named at all.

**The statistical rule.** A delta is not a finding until a paired bootstrap 95%
confidence interval over the instances, 20,000 resamples, excludes zero. Report
the interval with the delta, always. A comparison runs about 14 tests at once,
so one or two marginal significances in a table are the expected behaviour of
14 tests and not a result. Read the metrics that agree with each other, not the
one that happened to clear the bar.

**The WCC weights are parameters, and they were swept.** `ExploreEvaluator`
takes the weights in its constructor: 3.0 for main files and 2.0 for the other
core files. `calibrate_weights.py` sweeps the ratio from 1.0 to 10.0 with a
`ProcessPoolExecutor` and rescores every arm at each point. Measured: every
qualitative conclusion in this document holds across the whole grid. The shipped
3:2 therefore stands, and a reader who suspects the weights of producing a
result can re-run the sweep instead of arguing about it.

**One known resolution limit.** The test-file ground truth is 58 files in
total, across all 60 instances. The instrument cannot resolve a test-file recall
effect smaller than roughly 10 points. Any test-file claim from this benchmark
is therefore either large or unmeasurable, and the section below records one
that turned out to be the second.

## Integrity incidents

Two runs produced complete, plausible, and entirely invalid results. Each is
described with the signature that identifies it, because the signature is what
a future run checks itself against.

**1. The blind server, 2026-08-19 to 2026-08-24.** The harness unpacks each
instance's snapshot repository into a gitignored directory inside the bench work
tree. Until 0.6.1, `core.treewalk.walk_repo` took its file list from git
whenever git answered for the root. The enclosing repository answers for such a
root and lists zero paths for it, so every snapshot read as an empty repository:
no files to walk, no symbols to rank. Every 0.5.0 and 0.6.0 agentless figure
measured a dead structural server. Per-instance transcript and proof-log
forensics found it, not the scores. The fix was two-sided: 0.6.1's disowned-root
fallback in the server, and `git init` on every snapshot in the harness.
*Signature of a blind run:* a median of 2 to 3 MCP calls per instance, and
`capabilities` called in about 46 of 60 instances, where the agent asks the
server what is wrong.

**2. The toolless run, 2026-08-24.** A bare `uv sync` stripped the `mcp` extra
from the server's virtual environment, so the server died at spawn. The Claude
CLI treats a dead MCP server as non-fatal, so 120 instances ran to completion
and scored with no structural tools at all. The guardrail is
`probe_mcp_server.py`: it performs `initialize` and `tools/list` against the
exact server invocation the harness uses, and must answer 5. The run scripts
call it first and refuse to start otherwise. *Signature of a healthy run:* a
median of about 5 MCP calls per instance, and `capabilities` called in 0 of 60.

**3. Operational rules.** Each one below was learned by losing a run to it.

- Archive the result directories before any re-run. `--resume` silently
  no-ops over an existing row instead of replacing it.
- Run arms in parallel, never batches within one arm. Separate arms write
  separate directories; batches inside one arm race on the same append.
- Set `AGENTLESS_MCP_PROOF_DIR` to an absolute path. A relative path kills the
  logging proxy at spawn, and the run continues without proof logs.
- Never edit an arm's fingerprinted treatment files while that arm is running.
- A parse-failure row, meaning an instance with empty regions, is purged
  per-instance and resumed. It is not a CLI error and it is not a score.

## The arms

| Arm | Navigation offered |
|---|---|
| `claude_code` | the baseline: native tools only, `Grep` and `Glob` |
| `claude_code_agentless` | every native tool plus the five MCP tools, with the prompt steering toward them |
| `claude_code_agentless_forced` | the five MCP tools plus `Read`; native search withheld |
| `claude_code_agentless_hooked` | the free surface plus the `PreToolUse` gate that denies `Grep` and `Glob` until one agentless call, a `PostToolUse` marker, and per-instance `gate.jsonl` logging |
| `claude_code_agentless_hooked_deferred` | the hooked arm against the 0.6.2 deferred-schema server |
| `claude_code_agentless_gated_craft` | the deferred arm plus an emission-discipline prompt: line-anchored citations, root cause first, narrow spans, and text-search finds ranked last |

The forced arm is a measurement instrument, not a shipping configuration. It
answers whether tool availability or prompt wording produces the effect. The
hooked arm is the shipping configuration, because it defers native search
instead of removing it.

## Results

Healthy server, `agentless-mcp` 0.6.1, n=60, Sonnet, top 10, measured
2026-08-24. Every delta below is paired over the same 60 instances, and every
one quoted as a finding has a 95% interval excluding zero.

**Forced beat free-choice head to head.** Precision +0.062, recall +0.041, F1
+0.040, `hit_region_rate` +0.052, WCC +0.043, `recall@100` +0.011, with every
interval excluding 0. Both arms had the same prompt and the same server. The
only difference was whether native search was available. Prompt steering alone
did not produce the ordering discipline; withholding the alternative did. That
result is what the structural-first gate exists to reproduce, and it is why
0.6.2 dropped the eager-schema hint in favour of the gate.

**Against the grep-only baseline, the two agentless arms diverge.** Forced is
significantly better on WCC (+0.052, interval +0.009 to +0.095),
`hit_region_rate` (+0.057) and `recall@100` (+0.017). Free-choice is
significantly *worse* than the baseline on precision (-0.058) and `nDCG@100`
(-0.043). An agent that holds both tool sets and chooses freely does worse than
one that holds only `Grep`. Adding the tools without changing the ordering is
not a neutral change.

**The hooked arm is indistinguishable from forced, and keeps native search.**
The WCC delta against forced is +0.000 and the F1 delta is -0.004, neither
significant, while the arm made 174 post-unlock `Grep` and `Glob` calls across
35 instances. Against free-choice it is significantly better on `hit_file_rate`
(+0.040) and WCC (+0.043). It carries the best `hit_file_rate` of any arm
(0.645) and a `first_useful_hit` of 1.000. The gate denied a call in only 7 of
60 instances, so the cost of the mechanism is roughly one denied call per eight
sessions. One residual gap against forced remains: `recall@100` is -0.018
(interval -0.036 to -0.004). That gap is what the craft arm was built to
address.

**The 0.6.0 test-file claim is withdrawn, not confirmed.** 0.6.0 claimed that
widening test-file recognition "recovers test-file localization". The intervals
straddle zero on the healthy run: forced 36.2%, free 39.7%, baseline 39.7%. The
collapse that motivated the claim came from the blind run, where both agentless
arms scored 32.8% against the baseline's 44.8%, and it did not reproduce once
the server could see the repositories. Per the resolution limit above, the
instrument could not have resolved an effect this size in either direction.

**The mechanical part of that change is still verified.** Widening the
`is_test_path` predicate adds 11 ground-truth test files and loses none. That is
a property of the predicate, checked independently of the benchmark, and it does
not depend on any arm's score.

**The deferred and craft arms are in measurement at the time of writing.** Do
not quote a number for `claude_code_agentless_hooked_deferred` or
`claude_code_agentless_gated_craft` from this document. Their results land in
`CHANGELOG.md` and the README when they are final, and this section is updated
then.

## How to re-run

Work from the `swe-explore-bench` clone root. Its `RUNBOOK.md` is the
authoritative procedure; the steps below are the order the guardrails impose on
it.

1. Rebuild the server under test and confirm the client sees it. A repository
   fix is not live until the `uv tool install` is rebuilt.
2. Run the preflight: `python3 probe_mcp_server.py repos/<instance_id>`. It must
   print `5`. The run scripts, for example `run_061_agentless.sh`, refuse to
   start otherwise. Confirm separately that an `orient(map)` call against an
   unpacked snapshot lists that snapshot's files.
3. Archive the previous results. The convention is
   `archive_v<version>_<date>`, as in `archive_v061toolless_20260824`, and the
   directory name should say why the run was retired.
4. Run the arms in parallel, batches sequentially, as the run scripts do.
5. Score. `score_report.py` prints the per-arm means and the per-instance rows.
   `compare_060.py` produces the paired deltas and the bootstrap intervals.
   `calibrate_weights.py` sweeps the WCC weight ratio when a conclusion depends
   on a metric that uses them.
6. Check the health signature before reading any score: median MCP calls per
   instance, and the `capabilities` call rate. A median of 2 to 3 calls, or a
   `capabilities` rate near 46 of 60, means the server was blind and the run is
   void.

## The archived analysis record

Four earlier documents are kept, unedited, under
[`archive/`](archive/). Each carries a banner naming its successor. They are
history, not live guidance.

- [`archive/evidence.md`](archive/evidence.md) is the blind-server-era
  benchmark record. Its own correction banner declares every agentless figure
  invalid. It is kept because the record of what was run is worth more than a
  deleted file.
- [`archive/functional-assessment.md`](archive/functional-assessment.md)
  assessed the 0.3.0-era revision. Its M1 finding, that `--timeout` is a
  termination deadline rather than a hard wall-clock bound, is still cited from
  `core/sandbox.py`, which is why the file is archived rather than deleted.
- [`archive/audit-postmortem.md`](archive/audit-postmortem.md) explains why a
  nine-phase audit found defects that review had not. Its central rule is the
  one the guardrails above implement: **reproduce, do not read**, and a stated
  guarantee must name the gate that enforces it. Both integrity incidents were
  found by reproducing a run and reading its per-instance evidence, and both
  guardrails are now commands rather than intentions.
- [`archive/review-effectiveness-design.md`](archive/review-effectiveness-design.md)
  is the 0.3.1 effectiveness and design review. Its three headline findings were
  fixed on the day it was written, and the tool surface it describes is the
  retired v1 surface.

`docs/supply-chain-audit.md` is deliberately not in that list. It is the current
trust rationale for the exact grammar-pack pin, it is cited from
`pyproject.toml`, and it describes behaviour the code still has.
