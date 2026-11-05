# IRP analysis plan: the interleaved rerun of PH2C

The QMD design 1.1 template, filled in for this run before its first session. Design:
/var/tmp/claude-rust-port/qmd/design.md sections 1 and 2.4. Owner's approval (2026-11-05, plan
document section 9): section 2.4 with r = 2. All USD figures are Claude Code's API-equivalent
total_cost_usd. They are not a charge: every session runs through `claude -p` on the owner's Max
plan, as PH2C did.

```
RUN ID:            2026-11-05_irp-interleaved-ph2c      REGISTRY ENTRY: S/registry-entry.json
                   (main appends it to docs/analysis/runs/registry.jsonl; no line number yet)
WRITTEN:           2026-11-05 UTC by the IRP agent      PLAN SHA256: in the registry entry (a file
                                                        cannot hold its own hash)
QUESTION:          Does the UserPromptSubmit prompt-map hook (enrich) raise weighted_core_coverage
                   against native on H240 when the arms are interleaved in one window?
PRIMARY METRIC:    weighted_core_coverage of the top-10 regions, per task the mean of the two
                   replicates (direction: higher is better)
POPULATION:        H240 (240 tasks, file S/data/h240.jsonl, sha256
                   8c4178bf57a1ac6ee2668899de57ad8003d2c2842409eb7d6bc8ecb7db1a9dd8; instance-id
                   sha256 30422b870afcc0451d5400131757e650ca9efbddc1ddd4a93ca9adb229ef6bd7):
                   the bench300 rows not in flat60, in bench order. Chosen by the PH2C keep rule
                   (flat60 found the signal, so only tasks outside it can confirm it), never by
                   outcome. The same 240 tasks as PH2C's population H.
ARMS:              control native = claude_code_irp_native_r1, _r2: run_ab arm claude_code, no
                   --settings; bench commit b4afae8 (run_ab.py byte-identical to PH2C's 864e0d3,
                   sha256 29f9bfca...); Claude Code 2.1.318; model alias sonnet.
                   treatment enrich = claude_code_irp_enrich_r1, _r2: the same, plus
                   BENCH_BASELINE_SETTINGS = S/hooks/irp_prompt_map_settings.json (sha256
                   99f37164603f8975f45ee629af4234076ec86e4a51cd5b963917aa0a3135396c). The ONE
                   difference: the UserPromptSubmit hook hooks_product/prompt_map.py (sha256
                   4b261145...; rule P cap 2, 5 s deadline) names the map's top 2 files, once per
                   session, with the CLI 81d5a7e (S/bin/agentless-mcp, sha256 d83b456b...).
                   placebo native_placebo = claude_code_irp_native_placebo_r1, _r2: a second copy
                   of the control (env profile "native"), same build and settings.
REPLICATES:        r = 2 on all 240 tasks: 6 labels x 240 = 1,440 sessions.
SAMPLE SIZE:       n = 240. Source SD = 0.0675 (PH2C H enrich - native, n = 240, QMD
                   scripts/runcost.py, out/runcost.txt) to 0.0756 (placebo, two windows, n = 291,
                   QMD out/mvt_noise.txt). SD_r = SD / sqrt 2 = 0.0477 to 0.0535 (no task-by-arm
                   interaction). MDE = (1.96 + 0.8416) x SD_r / sqrt(240) = 0.0086 to 0.0097 at
                   power 0.80, alpha 0.05, 2-sided (QMD out/power.txt). Power at the PH2C effect
                   +0.0123: 0.95 to 0.98; at +0.006: 0.41 to 0.50. With an interaction variance of
                   0.004, SD_r is 0.083 to 0.088 and the MDE is 0.015 to 0.016 (derived).
                   Comparisons k = 1 (enrich - native); the placebo comparison is a validity check,
                   not a second treatment. Correction: none.
ORDER:             schedule S/data/schedule.jsonl, sha256
                   a68acc215bdb208563e56cccab7e99d1cc200761905fda47cc57deaec2a5aecc, seed 20261105
                   (fixed before the schedule was made; never drawn again), made by
                   make_schedule.py at b4afae8: 240 blocks = tasks in random order; the 6 sessions
                   (3 arms x 2 replicates) in random order inside each block; one window; 1 worker
                   (one session at a time). Failed rows run again after the last block, at most 2
                   times. A session that a plan usage limit ends is an instance error, not a score:
                   run_schedule.py stops the run (exit 75); RESUME=1 after the reset runs that row
                   first, and that re-run is not counted as a retry.
PASS RULE:         KEEP when the run is not VOID and (1) and (3) hold:
                   (1) the mean over the 240 tasks of [mean(enrich_r1, enrich_r2) -
                       mean(native_r1, native_r2)] WCC has a two-sided 95% bootstrap CI
                       (tvalib.mean_ci, seed 20261104, 10,000 resamples) with a lower bound above 0;
                   (3) the context delta per session (input + cache write + cache read, the same
                       per-task means) is +10.0% of the native mean or less (point estimate; the CI
                       is reported);
                   (4) the health checks below pass (else VOID).
                   DROP when the CI of (1) is all below 0, or (3) fails.
                   NOT CONFIRMED when the CI of (1) includes 0 and (3) holds.
                   Secondary metrics are reported. They never change the verdict.
PLACEBO RULE:      (2) VOID when the native_placebo - native WCC CI (the same computation as (1))
                   excludes 0 on either side. This contains the template rule: a placebo that
                   passes (1) has a CI above 0.
COST GATE:         context tokens per session (input + cache write + cache read) and output tokens
                   per session, per arm, from the runlogs. The priced cost is reported "cold":
                   input 1, cache write 1.25, cache read 0.1, output 5, with request 0's cache read
                   repriced at the write weight (request 0 from each stream).
COST CAP:          $60 API-equivalent (the sum of total_cost_usd over the six runlogs) and 1,500
                   sessions (attempts in the schedule log). run_schedule.py checks both before each
                   session and stops the run when either is reached (config "caps").
                   The real limit is the Max plan's usage limits: see ORDER.
HEALTH (VOID IF):  run.sh stops the run when one fails:
                   - preflight per arm: the worktree at b4afae8 with no tracked change; S/bin
                     matches its SHA256SUMS and COMMIT 81d5a7e; claude --version 2.1.318; the
                     frozen inputs match S/SHA256SUMS; the schedule sha256; the enrich hook
                     preflight (the hook command list, and the hook on PH2C's emitting ansible
                     prompt names the replay's two files, then stays silent on a second call);
                   - block 1, first-session check (first_check.py --instance) on each of the 6
                     labels: tools {Glob, Grep, Read}, mcp_servers [], output_style default, result
                     success, request 0 present, every run_manifest hook_settings equal to the
                     arm's file (null for native); enrich also: exactly 1 prompt-map gate row,
                     0 hook-error lines, no "internal" reason, the request-0 delta against the
                     native session of the same replicate in [+60, +130] when emitted, within
                     +-30 when silent;
                   - each enrich label: one emitted session that passes the same check (block 1,
                     else the next blocks, one at a time, at most 10);
                   - final, each label: 240 rows in top10.jsonl, 0 failed (the latest runlog row of
                     every task is scored), the session clauses above on every session, the
                     manifest clause; then 0 files with "is relative and was ignored".
                   analyze.py reads rule (4) from the six logs/<label>.done files. A label that
                   the operator accepts by hand after a failed final check fails rule (4).
                   Every session: env -u CLAUDECODE and the caller's CLAUDE*, AGENTLESS_*, BENCH_*
                   names; an absolute XDG_CACHE_HOME per label; TMPDIR=S/tmp;
                   DISABLE_AUTOUPDATER=1; CLAUDE_CODE_DISABLE_AUTO_MEMORY=1; gate and erosion logs
                   per label under S; a 4G systemd scope.
PREDICTIONS:       (1) WCC delta +0.003 (range -0.005 to +0.010); P(the CI is above 0) about 0.25.
                       Reason: the PH2C gain was the same where the hook was silent (+0.013) and
                       where it emitted (+0.012), and every later arm of the PH2 screen gained
                       +0.014 to +0.016. Native WCC 0.11 to 0.13 (PH2C H 0.1191).
                   (2) placebo delta within +-0.006, CI includes 0 (P about 0.95).
                   (3) context delta -3% to +3% of a native mean of 24,000 to 27,000 tokens
                       (PH2C H -2.0%); output 950 to 1,100 tokens per session.
                   Verdict: NOT CONFIRMED about 0.70, KEEP about 0.25, DROP or VOID about 0.05.
                   Recorded: silent-task delta CI includes 0; emitted minus silent CI includes 0;
                   position 1 minus position 6 CI includes 0; same-arm earlier minus later
                   replicate CI includes 0.
                   Health counts: 1,440 top10 rows; 0 failed rows after the retries; 0 to 5 failed
                   attempts; 0 to 2 usage-limit stops; emitted in 65% to 78% of the 480 enrich
                   sessions (PH2C H 170 of 240 = 70.8%); 0 to 6 deadline silences; request-0
                   delta +60 to +130 on every emitted session.
                   Noise: session SD of WCC within an arm 0.05 to 0.07; SD of the paired per-task
                   delta 0.045 to 0.090.
                   Cost: $39 to $47 API-equivalent in all (1,440 x $0.0274 to $0.0329, the PH2C
                   per-session means; derived); priced cold 17,000 to 19,500 tokens per session
                   (PH2C H measured 18,226 native and 18,003 enrich: an inline python3 pass over
                   the ph2c runlogs and streams, the analyze.py formula).
                   Time: about 4.3 h (derived below).
ANALYSIS SCRIPT:   S/analysis/analyze.py, sha256
                   80fec713ba56bab95bd7105219c29f5be2e7dc8f4ad7fdf32fa5d1ef02b71b83, with
                   S/analysis/tvalib.py, sha256
                   62c49920d0f025000ffa299fc20457afac480b71c94d5c14fa5294a31ff43c3d (PH2C's copy).
                   Both are in S/SHA256SUMS, which run.sh checks in the preflight.
                   Run on the dry-run output (S/dry: fake claude; planted in the enrich sessions
                   whose hook emitted: +0.2 pick probability per core region and +600
                   cache-read tokens) before the real run, outputs in S/analysis/test-out:
                   planted: (1) +0.1506 [+0.1160, +0.1850]; (2) -0.0123 [-0.0447, +0.0190];
                   (3) +470 tokens = +1.8% (derived plant: 690 x 340 / 480 emitted = +489);
                   verdict KEEP; silent tasks +0.0353 [-0.0121, +0.0847] (CI includes 0); emitted
                   tasks +0.1981; emitted minus silent +0.1628 [+0.0995, +0.2270]; position
                   1 minus 6 +0.0139 [-0.0385, +0.0659]. null (--treatment native_placebo
                   --placebo enrich): (1) -0.0123 [-0.0447, +0.0190] fails; (2) +0.1506 excludes
                   0; verdict VOID; silent and emitted CIs include 0. A second planted run gives
                   the same result.json (sha256 5e0e70166d612678...).
EXPLORATORY:       every analysis that this plan does not name is labeled "exploratory" in every
                   report.
```

## The recorded clauses (not gates)

- Silent-task delta: (1) restricted to the tasks where the hook is silent in both enrich
  sessions. Emitted-task delta: the same for the tasks where it emits in both. Tasks with one of
  each are "mixed" and are counted only.
- Emitted minus silent: the difference of the two group means, with a bootstrap that resamples
  each group (numpy, seed 20261104, 10,000).
- Position effect: per block, WCC of the position-1 session minus WCC of the position-6 session
  (240 pairs; MDE about 0.0145 at SD 0.08, QMD out/power.txt). Also per block, the mean over the
  3 arms of WCC(earlier replicate) - WCC(later replicate). Positions are the schedule's; the
  count of rows that ran again (retries, usage-limit re-runs) is reported.
- The SDs for later power computations: the SD of the paired per-task delta, and each arm's
  session SD (SD of r1 - r2 over the tasks, divided by sqrt 2).

## Secondary metrics

- The 17 stored metrics on the per-task replicate means, treatment - native and placebo - native,
  with CIs; the count of metrics better and worse with a CI that excludes 0.
- score_report.py --top-k 10 --all on each replicate pair (native_rk against enrich_rk and against
  native_placebo_rk), the 21 metrics with score_report's paired bootstrap, as PH2C read them. The
  analysis stops when score_report's WCC delta differs from the stored rows' delta by more than
  its 4-decimal print.
- Per arm per session: context, output, turns, priced cold tokens, total_cost_usd
  (API-equivalent); the priced-cold delta with its CI; the run totals (sessions, error rows,
  API-equivalent USD, attempt outcomes).

## Time (derived)

1,440 sessions x (10.4 to 10.7 s, the PH2C mean wall per session, QMD out/runcost.txt, plus
0.22 to 0.25 s per row, the dry run's start-to-start time with a fake session of about 0 s)
= 4.26 to 4.38 h, plus the time a usage-limit stop waits for the reset. PH2C ran its 600
sessions in 1 h 46 min (10.6 s per session, logs/run.out).

## Launch

From the shell of main (the dry run must pass first):

    /var/tmp/claude-rust-port/irp/run.sh > /var/tmp/claude-rust-port/irp/logs/run.out 2>&1

After a usage-limit stop (exit 75), when the limit has reset:

    RESUME=1 /var/tmp/claude-rust-port/irp/run.sh >> /var/tmp/claude-rust-port/irp/logs/run.out 2>&1

Then, with the bench venv, from S/analysis: `python analyze.py` (writes S/analysis/out).

MAIN'S RULINGS (2026-11-05, before the first session, on IRP's open questions):
- The placebo rule is two-sided: VOID when the placebo - native CI excludes 0 on either side. Confirmed.
- DROP and NOT CONFIRMED as defined above. Confirmed.
- The position effect is block position 1 minus position 6, with the earlier-minus-later replicate contrast per
  arm. Confirmed; recorded, not a gate.
- A label accepted by hand after a failed final check fails health rule (4): the run is VOID. Confirmed.
- In stage 2, a failed row runs again right after its block. Confirmed.
- Every USD figure is Claude Code's API-equivalent total_cost_usd, not a charge: the owner has a Max plan and no
  API key. A usage-limit stop (exit 75) is resumed with RESUME=1 after the reset.
