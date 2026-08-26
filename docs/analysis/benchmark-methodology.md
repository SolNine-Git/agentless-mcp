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

Three runs produced complete, plausible, and entirely invalid results. Each is
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

It recurred on 2026-08-25 through a door the guardrail did not cover. Pinning
an arm to a commit with `git worktree add` creates a worktree whose `.venv` is
built from scratch and therefore carries no `mcp` extra. Priming it with a CLI
call succeeds -- the CLI does not need `fastmcp`, only the server path does --
so the worktree looked ready. Twenty-seven instances then ran against a server
that exited after `initialize`, fell back to `Bash` and `Grep`, and scored.
The result read as a plausible finding: +74% turns, +37% cost, -36% tokens per
turn. Every number was the fallback. *Cheapest post-hoc check:* an arm with
zero `tools/list` rows in its proof log never had tools, whatever it scored --
`grep -c '"tools/list"' mcp_proof_<tag>/*.mcp_calls.jsonl`.

**3. The moving tree, 2026-08-25.** An arm was pointed at the live checkout
rather than a pinned worktree. Another session committed review fixes into
that checkout eighteen minutes into the run, and because the arm spawns its
server from the checkout's editable install, instances before and after the
edit ran different code. Six of twenty-four instances ran the committed
build and the rest ran a tree that was still changing. *Rule:* both arms of a
comparison are pinned with `git worktree` to explicit commits, always. The
control arm was pinned and the treatment arm was not, which is how a
comparison acquired a moving side without anyone choosing it.

**4. Operational rules.** Each one below was learned by losing a run to it.

- Archive the result directories before any re-run. `--resume` silently
  no-ops over an existing row instead of replacing it.
- Run arms in parallel, never batches within one arm. Separate arms write
  separate directories; batches inside one arm race on the same append.
- Set `AGENTLESS_MCP_PROOF_DIR` to an absolute path. A relative path kills the
  logging proxy at spawn, and the run continues without proof logs.
- Never edit an arm's fingerprinted treatment files while that arm is running.
- A parse-failure row, meaning an instance with empty regions, is purged
  per-instance and resumed. It is not a CLI error and it is not a score.

## What each tier can answer

Three instruments measure three different things, and the failure mode is
letting a cheap one answer an expensive one's question.

| Claim | Instrument | Why it is valid for that claim |
| --- | --- | --- |
| the ranking changed | `loc-bench-harness` | scores file ranking with no flattening step; 47 s; records compare byte for byte |
| the output got denser or cheaper | direct measurement | symbols per rendered token, character counts, token pins -- it measures the thing itself |
| an agent does better | the agentic arms | an agent chooses what to read; no cheaper tier models that |

**`agentless_deterministic` is a guard, not a scoreboard.** It is the only
tier that reads the map's JSON, which is why it caught the 2026-08-25 defect
where a focused `map --json` returned `"files": []` for a file it had ranked
first. Assert on it: no instance with `num_regions == 0`, no `error` rows, no
instance losing a gold file it previously found. Do not read its `recall`,
`precision` or `hit_file_rate` as evidence about a rendering change, for the
reason below.

**The flattening confound.** The explorer turns a map into a flat ranked
region list by draining the top-ranked file's symbols before moving to the
next, then slicing at top-K. Region *order* follows file rank, but the number
of slots each file consumes is its *symbol count*, which carries no relevance
information. A symbol-rich file at rank 1 starves a gold file at rank 3 out of
the cut. Any change to how densely the map renders is therefore measured by
this arm as though it were a ranking change.

Measured 2026-08-25 on the 60-instance pilot, two commits whose rankings are
identical (`loc-bench` 50/50):

| Metric | default order | `--interleave` |
| --- | --- | --- |
| recall | denser build +0.004 | sparser build +0.010 |
| precision | denser build +0.017 | sparser build +0.008 |
| `hit_file_rate` | sparser build +0.017 | sparser build +0.008 |
| `context_efficiency` | denser build +0.007 | sparser build +0.039 |

The two modes disagree on nearly every metric. Same commits, same instances,
same maps; only the flattening rule differs. `--interleave` round-robins one
symbol per file, so a budget of k regions can reach k distinct files -- which
matters because ground truth spans 4 to 12 files per instance, a spread strict
rank order cannot cover at small k. Against the agentic arm on the same build
and instances, interleaved is the better proxy on every metric measured
(`hit_file_rate` 0.369 against 0.067, recall 0.367 against 0.032), so it is
the default worth running. Both remain weak proxies at r around 0.37.

**Report both modes or neither.** A single-mode region score is a choice that
changes which build wins.

**Two metric-hygiene rules.** Report the excluding-bulk-read aggregate beside
every headline number: about a third of instances have a gold region covering
90% or more of its file, so reading more scores higher by construction. And
`weighted_core_coverage` is not a proxy for agent behaviour in either mode --
measured at 0.020 interleaved and -0.002 in default order.

**The open gap.** The agentic tier has no noise floor recorded here. Until a
same-arm replicate is run and written into this document, no agentic delta is
interpretable -- including a favourable one.

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
(interval -0.036 to -0.004). The deferred arm closed that gap; see below.

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

**Deferring the schemas costs nothing and closes the hooked arm's one gap.**
The deferred arm (60/60, zero errors) is inside every interval against the
hooked arm except `recall@100`, which moves +0.017 *in the deferred arm's
favour* -- one significant result among roughly 22 tests, so read it as "no
cost, possibly a small benefit". Its `recall@100` of 0.1345 matches forced
(0.1351, delta -0.001, not significant), so the hooked arm's -0.018 gap does
not survive deferral. Against the baseline it is significantly better on
`hit_region_rate` (+0.041), `recall@100` (+0.016) and `recall@500` (+0.033),
and significantly worse on precision (-0.052), the cost of mixing native
search back in after unlock. This is the configuration 0.6.2 ships.

**The emission-discipline prompt made the agent worse, and is not shipped.**
The craft arm added a prompt block asking for line-anchored citations, root
cause first, narrow spans, and text-search finds ranked last -- on top of the
deferred configuration, changing nothing else. Against deferred it lost
`hit_file_rate` (-0.058, interval -0.102 to -0.017), `hit_region_rate`
(-0.054, interval -0.094 to -0.017) and `recall@100` (-0.020, interval -0.040
to -0.005), with no significant gain anywhere; against forced it shows the
same three losses. Its nominal precision and lowest noise rate did not reach
significance. The reporting constraints appear to buy caution with coverage:
the agent cites fewer files and misses more of the core. The prompt is
recorded here as a negative result and does not ship in any surface.

## How to re-run

Work from the `swe-explore-bench` clone root. Its `RUNBOOK.md` is the
authoritative procedure; the steps below are the order the guardrails impose on
it.

1. Rebuild the server under test and confirm the client sees it. A repository
   fix is not live until the `uv tool install` is rebuilt.
2. Pin every arm to a commit with `git worktree add --detach <dir> <sha>`, and
   point the arm at it with `AGENTLESS_PROJECT`. Never point an arm at the live
   checkout: another session editing it mid-run splits the arm across two
   builds, as incident 3 records.
3. Give each worktree the server extra -- `uv sync --project <dir> --extra mcp`
   -- because a fresh worktree's `.venv` has none, and a CLI smoke test passes
   without it.
4. Run the preflight against each worktree:
   `python3 probe_mcp_server.py repos/<instance_id> <worktree>`. It must
   print `5`. The run scripts, for example `run_061_agentless.sh`, refuse to
   start otherwise. Confirm separately that an `orient(map)` call against an
   unpacked snapshot lists that snapshot's files.
5. Archive the previous results. The convention is
   `archive_v<version>_<date>`, as in `archive_v061toolless_20260824`, and the
   directory name should say why the run was retired.
6. Run the arms in parallel, batches sequentially, as the run scripts do.
7. Score. `score_report.py` prints the per-arm means and the per-instance rows.
   `compare_060.py` produces the paired deltas and the bootstrap intervals.
   `calibrate_weights.py` sweeps the WCC weight ratio when a conclusion depends
   on a metric that uses them.
8. Check the health signature before reading any score: median MCP calls per
   instance, and the `capabilities` call rate. A median of 2 to 3 calls, or a
   `capabilities` rate near 46 of 60, means the server was blind and the run is
   void. Check `tools/list` first: an arm with zero of them in
   its proof log never had tools at all, and no score it produced is about the
   build. `grep -c '"tools/list"' mcp_proof_<tag>/*.mcp_calls.jsonl`.
9. Match the tier to the claim before reporting anything. See *What each tier
   can answer*: region-level scores from `agentless_deterministic` are not
   evidence about a rendering change, in either flattening mode.

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
