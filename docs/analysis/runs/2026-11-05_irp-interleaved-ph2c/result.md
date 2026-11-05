# Result: the interleaved rerun of PH2C

Registry id: `2026-11-05_irp-interleaved-ph2c` (runs/registry.jsonl). Plan: [plan.md](plan.md), sha256
e4aee592..., committed at 04:24:03 UTC, 35 seconds before the first session. Analysis script sha256
80fec713..., output `result.json` sha256 5d0f9e92.... A second run of the script gave the same output.

## Verdict

KEEP. All four rules of the plan hold.

| Rule | Measured | Holds |
|---|---|---|
| (1) WCC, enrich minus native, CI above 0 | +0.0119 [+0.0043, +0.0199] | yes |
| (2) placebo, native_placebo minus native, CI includes 0 | -0.0009 [-0.0068, +0.0053] | yes |
| (3) context per session at most +10% | -5.9% | yes |
| (4) health: 6 final checks pass | 1,440 scored, 0 failed, 0 retried | yes |

Each value is the mean over the 240 tasks of the per-task mean of 2 replicates, with a paired bootstrap 95%
interval (seed 20261104, 10,000 resamples).

## The numbers

| Measure, per session | native | enrich | native_placebo | enrich minus native |
|---|---|---|---|---|
| weighted_core_coverage (WCC) | 0.1186 | 0.1305 | 0.1177 | +0.0119 [+0.0043, +0.0199], +10.0% |
| context tokens | 26,847 | 25,265 | 25,927 | -1,582 [-2,837, -511], -5.9% |
| priced cost, cold (input-equivalent tokens) | 18,043 | 17,926 | 17,769 | -117 [-522, +258] |
| output tokens | 1,020 | 1,009 | 1,010 | |
| turns | 4.70 | 4.58 | 4.63 | |

The secondary metrics (reported, not gates): on the per-task means, 9 metrics are better for enrich and none
worse (f1, recall, WCC, and recall and nDCG at 100, 300 and 500). score_report.py on replicate 1 alone finds no
metric better or worse. On replicate 2 alone it finds 9 better and none worse. The placebo moves no metric on the
per-task means.

## The recorded clauses (not gates)

| Clause | Predicted | Measured |
|---|---|---|
| silent tasks (the hook silent in both enrich sessions), n = 69 | CI includes 0 | -0.0018 [-0.0116, +0.0076] |
| emitted tasks (the hook emitted in both), n = 170 | | +0.0175 [+0.0075, +0.0282] |
| emitted minus silent | CI includes 0 | +0.0193 [+0.0057, +0.0337]: the prediction failed |
| block position 1 minus position 6 | CI includes 0 | -0.0086 [-0.0184, +0.0007] |
| same arm, earlier minus later replicate | CI includes 0 | -0.0037 [-0.0088, +0.0010] |

One task had the hook emit in one enrich session and stay silent in the other. Of the 480 enrich sessions, the
hook emitted in 341 (71%). It stayed silent in 74 because the issue text names no file or symbol, in 62 because
no named seed resolved in the map, and in 3 because the map passed its 5-second deadline.

## What this changes about PH2C

PH2C ran the native arm and then the enrich arm on the same 240 tasks. Its gain was the same where the hook stayed
silent (+0.013) as where it emitted (+0.012). So the order of the arms or the time of the run could explain it.
This run put the arms in a random order inside each task's block, in one window. Here the silent tasks gain
nothing, the gain is in the tasks where the hook put files into the prompt, and the placebo arm does not move.
The PH2C gain on the silent tasks came from the order or the time of that run, not from the hook. The hook's own
gain, +0.0119 on all tasks and +0.0175 on the tasks where it emits, is now measured with that confound removed.

## Limits

1. The split into emitted and silent tasks is not random. The hook emits when the issue text names a path or a
   symbol that the map resolves, so the two groups differ in kind. Inside each group the comparison is paired and
   the arm order is random. The difference between the groups shows where the gain is, not why.
2. The gain is small in absolute terms: +0.012 on a native WCC of 0.119. One replicate alone does not show it at
   score_report's test (limit of power). The per-task mean of 2 replicates, the primary of the plan, shows it.
3. WCC is a retrieval measure: how much of the core regions (the code a reference trajectory read) the agent's
   ranked context covers. It does not measure whether the agent writes a better patch. Methods (a) and (b) of the measurement-standard plan measure that. Owner answer 5
   of that plan: method (b) uses native, native_placebo and enrich, because this run keeps enrich.
4. One task set (H240), one model (claude-sonnet-5-5), one Claude Code version (2.1.318), one build (81d5a7e).
   The fresh-task set (F548) has not run.
5. The block-position contrast is -0.0086 with an upper bound of +0.0007, close to 0: sessions later in a block
   may score a little higher. The random order inside each block spreads such an effect evenly over the arms, so
   it does not bias the verdict. A run with the arms in a fixed order would carry it into the result.
6. The archive (owner answer 6) is not set up. The result files exist only on the working disk.

## The predictions against the result

| Prediction (plan) | Measured | Outcome |
|---|---|---|
| WCC delta +0.003, range -0.005 to +0.010 | +0.0119 | failed (above the range) |
| verdict NOT CONFIRMED about 0.70, KEEP about 0.25 | KEEP | the less likely outcome |
| placebo delta within +-0.006, CI includes 0 | -0.0009, CI includes 0 | held |
| context delta -3% to +3% | -5.9% | failed (below the range) |
| native context 24,000 to 27,000 tokens | 26,847 | held |
| output 950 to 1,100 tokens per session | 1,009 to 1,020 | held |
| emitted in 65% to 78% of the enrich sessions | 71% | held |
| 0 to 6 deadline silences | 3 | held |
| session SD of WCC within an arm 0.05 to 0.07 | 0.043 to 0.054 | 2 of 3 arms below the range |
| SD of the paired per-task delta 0.045 to 0.090 | 0.063 | held |
| priced cold 17,000 to 19,500 tokens per session | 17,769 to 18,043 | held |
| $39 to $47 API-equivalent | $38.11 | failed (just below) |
| about 4.3 h | 4 h 16 min | held |

The WCC and verdict predictions failed because the plan assumed that the PH2C gain was all order or time. The
context prediction used PH2C's -2.0%; this run measured a larger reduction.

## Cost and time

1,440 sessions, $38.11 API-equivalent (Claude Code's `total_cost_usd`; not a charge, because the runs use the
owner's Max plan). The cap was $60. The window was 04:24:38 to 08:40:19 UTC on 2026-11-05. No session hit a
usage limit.

## Reproduce

From the run directory `/var/tmp/claude-rust-port/irp` (S):

```bash
sha256sum -c SHA256SUMS                       # the 12 frozen inputs
cd analysis && <bench>/.venv/bin/python analyze.py   # writes analysis/out/result.json and result.txt
python3 pm/finalize_registry.py               # the registry line; sha256 06d613c8... for the script
```

`<bench>` is the swe-explore-bench clone whose virtual environment ran the analysis. The script analyze.py reads
the 6 label directories under S/results and the 6 `.done` files under S/logs.
