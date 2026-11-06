# QFA analysis plan: method (a) on H240

The template of plan 1.1 (docs/plans/2026-11-05_measurement-standard-and-code-quality.md on main 14f8a95), filled in for this run before its first writer call. Design: plan sections 4.1, 4.2, 4.4 (L1), 4.5, 4.6, and main's rulings of 2026-11-06 (the primary on IRP's interleaved contexts). Owner approval: "ok lets go with a, give it a shot" (2026-11-06, the full method (a) run). All USD figures are Claude Code's API-equivalent total_cost_usd. They are not a charge: every writer call is a `claude -p` call on the owner's Max plan. W = /var/tmp/claude-rust-port/pqp, B = W/bench (the bench clone), F = W/full-a, S = F/run (the state of the real run), I = /var/tmp/claude-rust-port/irp.

Why method (a): the pilot found method (b) at ceiling. Native, native_placebo and enrich each resolved 9 of 10 tasks, with a discordant share of 0/10, and 4 of 10 tasks gave a byte-identical patch in all three arms (ledger, PILOT DONE). The model goes to the fix in a few calls, so exploration has nothing to change there. Method (a) holds the writer fixed and changes only the explored context, so it is the instrument of this question. Why IRP's contexts: PH2C ran native and then enrich, and IRP showed that the PH2C gain on silent tasks came from order or time. IRP ran native, enrich and native_placebo interleaved in one window, with 2 replicates each.

```
RUN ID:            2026-11-06_qfa-method-a-h240
                   REGISTRY ENTRY: F/registry-line.json (main appends it to docs/analysis/runs/registry.jsonl)
WRITTEN:           2026-11-06 23:31 UTC by the QFA agent  PLAN SHA256: in the registry entry
QUESTION:          Does the explored context of the prompt-map hook (IRP enrich, interleaved) raise the share of
                   H240 tasks that a fixed patch writer resolves, against the IRP native context?
PRIMARY METRIC:    resolved (plan 4.1): every FAIL_TO_PASS test passes and every PASS_TO_PASS test still passes,
                   graded in a fresh container from the pristine image. Empty patch, failed apply and timeout
                   count as not resolved. Direction: higher is better.
POPULATION:        H240 (I/data/h240.jsonl, 240 tasks, sha256
                   8c4178bf57a1ac6ee2668899de57ad8003d2c2842409eb7d6bc8ecb7db1a9dd8; instance-id sha256
                   30422b870afcc0451d5400131757e650ca9efbddc1ddd4a93ca9adb229ef6bd7; both equal to IRP's registry
                   entry): the bench300 rows not in flat60, 130 Verified, 63 Pro, 47 Multilingual. Chosen before
                   this run, never by outcome. The 7 H240 Pro tasks outside SWE-bench Pro V2 have no grader and
                   leave before stage 0 (plan Q4). Stage 0 per remaining task before its rows: the gold patch must
                   resolve and the empty run must not. A task that fails leaves the analysis with its reason in
                   S/excluded.jsonl; there is no replacement. The 8 pilot tasks in H240 are graded again (the
                   grader changed in bench 3ad46c3; the Verified images now come from the Epoch registry).
ARMS:              Method (a): the arms are explored contexts; one writer reads each. Writer: `claude -p`,
                   claude-sonnet-5-5, --effort high, every tool off (--tools ""), --setting-sources project,local,
                   --strict-mcp-config, --no-session-persistence, CLAUDE_CODE_DISABLE_CLAUDE_MDS=1, Claude Code
                   2.1.318, the prompt of quality/gen_patches.py without the gold "Files that need to be
                   modified" section, the issue text of B/issue_map.json, one 2,000-line context cap. Bench:
                   W/bench branch quality-full-a at 0ae1da309ea59c2cc10502f81ab8623387b046eb.
                   control irp_native_r1: the top-10 regions of IRP's native replicate 1 (S/inputs/
                   irp_native_r1.top10.jsonl, sha256 0559d6d4321b1a80c783253fe5da4a33b34afbd5a29f2b242cc3372f07e5981c).
                   treatment irp_enrich_r1: IRP's enrich replicate 1 (sha256
                   8a2e0505d9143c50a93b7adc129cce50ea2649beb2df1ff75997790e1de285b5). The ONE difference: the
                   exploration session ran with the UserPromptSubmit prompt-map hook.
                   placebo (writer) writer_placebo: the irp_native_r1 context written a second time.
                   placebo (exploration) irp_native_placebo_r1 (sha256
                   65c0a8e62c196b2482a04a560450e321a4ca8b3a84d951d05e772127aa7e9f15): a second native exploration
                   in the same interleaved window.
                   replication: irp_enrich_r2 (457dee5d76bffd4ec335ec90e92e3aa09086ab38dea35c25c1cb8105f0a14d32)
                   against irp_native_r2 (4dbb2951f6da9d5053ea7a824d08110b9d8f73bfb80d72bc4f5a63bb23d9029e).
                   L1 context also: irp_native_placebo_r2 (6bc1b8f757273482d9f885f39c8b1b9a1ebfd341c143d5183acfb54206a9b003).
                   anchors: no_context (the issue only) and oracle (the core ground-truth regions).
                   9 labels: 2,160 writer rows planned, 2,097 at most after the 7 Pro tasks leave.
REPLICATES:        At the explorer level r = 2: each IRP arm has replicates r1 and r2, and the run writes from
                   both. The writer is r = 1 per context, except the writer placebo, a second writer call on
                   irp_native_r1 for every task. So the run measures the writer's own discordant share on all n
                   tasks and the explorer's replicate variation on the native arm.
SAMPLE SIZE:       n = tasks that pass stage 0: predicted 229 (225 to 233). Source PD: the pilot (n = 10):
                   2/10 enrich against native, 1/10 writer placebo, 1/10 exploration placebo. MDE at n = 229,
                   power 0.80, two-sided 0.05 (Connor normal formula of plan 4.5; QMD scripts/power.py, output
                   F/tmp/power_qfa_h240.txt): PD 0.10: 5.8 points; PD 0.15: 7.1; PD 0.20: 8.2. The exact test has
                   power 0.76 to 0.77 at those D. Power at D = 3 points: 0.23 at PD 0.10, 0.14 at PD 0.20; at D = 5
                   points: 0.61 at PD 0.10, 0.35 at PD 0.20. Comparisons k = 1 (the primary). The placebos are
                   validity checks. The replication and the pooled sign test are reported and never change the
                   verdict; no correction.
ORDER:             schedule S/schedule.jsonl, sha256
                   2c73ca6d590cbb626832180c90b90c26f5be1d0a7c374038afc7987dc4083e6d, seed 20261105 (fixed before
                   the schedule existed), made by `quality.full_a schedule` at 0ae1da3 from H240 in its file order:
                   240 blocks = tasks in seeded order; the 9 labels in seeded order inside each block
                   (pilot_run.build_schedule). One window. The writer calls start in schedule order (a call waits
                   for every earlier row of a task that stays in); at most 4 writer calls, 4 grading containers
                   and 2 check runs at once; stage 0 runs two blocks ahead of the row grades; images are pulled at
                   most 8 blocks ahead. A failed step runs again at once, at most 3 attempts; a third failure stops
                   the run (stage 0: the task leaves). A usage limit stops the run (exit 75); the same command
                   resumes it with only the steps that are not done; a usage-limit attempt is not a failure.
PASS RULE:         On the n tasks, b = tasks that irp_enrich_r1 resolves and irp_native_r1 does not, c = the
                   reverse. Exact McNemar, two-sided: p = min(1, 2 x P(X <= min(b, c))), X ~ Binomial(b + c, 1/2).
                   KEEP when p < 0.05 and b > c. DROP when p < 0.05 and b < c. NOT CONFIRMED when p >= 0.05.
                   Reported with it: D = (b - c) / n with a 95% bootstrap CI over tasks (seed 20261105, 10,000
                   resamples), PD = (b + c) / n, both resolved shares. The replication (the same test on r2) and
                   the pooled sign test (per task, resolved count of irp_enrich_r1 and r2 minus that of
                   irp_native_r1 and r2, from -2 to +2; exact two-sided sign test on the nonzero differences;
                   the same rule names) are reported. They never change the verdict.
PLACEBO RULE:      The pass rule on (writer_placebo against irp_native_r1) and on (irp_native_placebo_r1 against
                   irp_native_r1). VOID when either gives KEEP or DROP (two-sided). Under H0 the exact test
                   rejects at 0.030 to 0.036 (n = 229, PD 0.10 to 0.20), so a null run is VOID by a placebo with
                   probability about 0.06 to 0.07.
COST GATE:         Reported per label, not a rule (in method (a) the context is the treatment's input): context
                   tokens per call (input + cache write + cache read), output tokens per call, and the priced
                   cost cold (input 1, cache write 1.25, cache read 1.25: each call is request 0, rule 4).
COST CAP:          $120 API-equivalent: the summed total_cost_usd of every writer attempt, checked before each
                   writer call (at most 3 calls in flight can pass it). At most 3 attempts per writer row. Derived
                   need: $62 (the pilot's per-dataset means: Verified $0.0243, Multilingual $0.0241, Pro $0.0461
                   per call, times 130, 47 and 56 tasks x 9) to $65 (the pilot mean $0.0308 x 2,097).
HEALTH (VOID IF):  the run stops (run_full_a.sh exit code) on: a health check before a dispatch round (free -g
                   available >= 24 GB, / free >= 150 GB, PQP images <= 150 GB, the PQP API answers, no container
                   that a running grade does not own) (3); the pinned inputs once per block (claude --version
                   2.1.318 and not the fake, the sha256 of every file in S/schedule_meta.json, the bench clone at
                   0ae1da3 with no tracked change, the V2 checkout at 66f9276 with no change) (3); the cap (4); a
                   grade or stage-0 container outside 6 GiB memory and swap.max 0 (5); 3 failed attempts of one
                   step (6). A stop is safe: the same command resumes the run. analyze.py rules (VOID if one
                   fails): every included task has a grade and a checks row for all 9 labels (else INCOMPLETE, no
                   verdict); every included task has gold resolved and empty unresolved in stage 0; 0 grade rows
                   with status error and every container row at 6 GiB and swap 0; every writer envelope with
                   model_usage = {claude-sonnet-5-5} and num_turns 1; the state files that analyze.py reads match
                   their schedule sha256; no STOPPED marker. Every command of the run: the separate PQP podman
                   storage and socket only, the grader never pulls (each pull is a row of W/images.jsonl; only
                   listed IDs are removed), the isolation proof (owner default storage counts and sorted-id sha256
                   before and after, S/isolation.txt), XDG_CACHE_HOME, TMPDIR and the gate and erosion logs under
                   W, PYTHONDONTWRITEBYTECODE=1, DISABLE_AUTOUPDATER=1, no process that inherits the run's lock
                   descriptor.
PREDICTIONS:       see the section below; written before the run.
ANALYSIS SCRIPT:   F/analyze.py, sha256 8827e9e49b0746a028c054f39e56e44686d1f2aed54f61a3704ab6035226af28,
                   standard library only; F/test_analyze.py, sha256
                   da2c03684cdb47ce37338242a445c56530d3fa5a98b00f2fd23a77d6b13a3feb (20 tests pass, run with the
                   bench venv for scipy). Both in F/SHA256SUMS, which run_full_a.sh checks before each run. Run on
                   the dry-run output (F/dry) before the real run: see "The dry run" below.
EXPLORATORY:       every analysis that this plan does not name is labeled "exploratory" in every report (for
                   example the split by hook emission, by dataset, by language).
```

## What analyze.py prints (every number of this plan)

`python F/analyze.py --state S` writes S/analysis/result.json and result.txt:
1. Counts: tasks, excluded tasks with their reasons, n.
2. Health: the 6 rules above, each with its detail.
3. Primary: b, c, n, PD, D with its 95% bootstrap CI, both resolved shares, exact p, the rule; the verdict (KEEP, DROP, NOT CONFIRMED, VOID, INCOMPLETE).
4. Placebos: the same numbers for the writer placebo and the exploration placebo, each with its rule.
5. Replication: the same numbers for irp_enrich_r2 against irp_native_r2, with its rule (reported only).
6. Pooled sign test: n, positive, negative and zero tasks, the distribution of the difference (-2 to +2), its mean, exact p, the rule (reported only).
7. L1 (plan 4.4): on the 6 IRP explored contexts of each task (the anchors and the writer placebo stay out), each pair with a different resolved outcome and a different WCC (the weighted_core_coverage stored in each IRP top10 row) scores 1 when the resolved one has the higher WCC. c = concordant pairs / informative pairs, pooled over tasks; 95% bootstrap CI over tasks (seed 20261105, 10,000 resamples, percentile); H0: c = 0.5; prediction 0.55 to 0.70. Secondary of L1: conditional logistic regression with task strata (exact likelihood, Newton-Raphson, Wald CI) of resolved on WCC (odds ratio per 0.1 WCC) and, in a second fit, on "a non-test file of the reference fix is in the context" (S/inputs/reference_files.json). A fit with separation is reported as not estimable.
8. Resolved per label: k/n, Wilson 95% CI, the status counts, per dataset.
9. The deterministic checks of plan 4.1 per label: findings per check and severity, patches with each finding, not_checked rows per check (language coverage), empty patches, since-pass apply failures, median files, lines added and removed, patches with a changed non-test file outside the reference fix.
10. The cost gate per label (calls, context, output and priced-cold tokens per call, USD) and the run total.

## Predictions (before the run)

| Item | Point | Range | Reason |
|---|---|---|---|
| n after stage 0 | 229 | 225 to 233 | 7 Pro tasks leave (no V2); the pilot's stage 0 passed 10/10; Epoch images are rebuilt |
| resolved, irp_native_r1 | 0.52 | 0.40 to 0.65 | pilot native contexts 7 to 8 of 10; SWE-Explore restricted-context writers 0.48 [S1] |
| resolved, oracle | 0.62 | 0.50 to 0.75 | pilot 8/10; SWE-Explore oracle 0.597 [S1] |
| resolved, no_context | 0.32 | 0.20 to 0.45 | pilot 4/10 |
| primary D (enrich_r1 - native_r1) | +0.5 points | -3 to +4 | IRP: enrich WCC +0.012 on a native 0.119; the WCC-to-resolved link is unknown |
| primary PD | 0.14 | 0.08 to 0.20 | pilot 2/10 |
| writer placebo PD, D | 0.10, 0 | 0.06 to 0.15; D within +-3 | pilot 1/10 |
| exploration placebo PD, D | 0.12, 0 | 0.06 to 0.18; D within +-3 | the same window; pilot 1/10 |
| verdict | NOT CONFIRMED | P: NOT CONFIRMED 0.83, KEEP 0.06, DROP 0.03, VOID 0.08 | MDE 6 to 8 points against a predicted D under 1 point |
| replication (r2) | NOT CONFIRMED | D -3 to +4 points, P(NOT CONFIRMED) 0.85 | as the primary |
| pooled sign test | NOT CONFIRMED | nonzero differences in 20% to 30% of tasks; P(NOT CONFIRMED) 0.85 | |
| L1 c | 0.60 | 0.55 to 0.70; CI excludes 0.5 with P 0.6 | WCC rewards the regions that the reference trajectory read |
| L1 informative pairs | 500 | 300 to 750, in 60 to 110 tasks | about 35% of tasks with mixed outcomes x 6.5 pairs, less WCC ties |
| clogit OR per 0.1 WCC | 1.2 | 1.0 to 1.6 | |
| clogit OR, reference file in context | 2.0 | 1.3 to 3.5 | |
| apply_failed share per explored label | 0.15 | 0.08 to 0.22 | pilot 14/90 |
| cost | $63 | $50 to $85 | derived above |
| context / output tokens per call | 4,000 / 1,700 | 3,000 to 5,500 / 1,200 to 2,500 | pilot means 4,062 / 1,682 |
| stops | 0 health, 0 integrity, 0 to 2 usage limit | | pilot: 0 failed attempts in 270 steps |
| failed attempts | 3 | 0 to 12 | harness errors are retried |
| image pulls | 231 | | 130 Epoch, 55 V2 (teleport present), 46 Docker Hub (gson present); never paced (46 < 90) |
| peak listed image size | 60 GB | 30 to 100 GB | 8-block lookahead; about 23 GB of pilot images at the start |
| wall time | 5 h | 3.5 to 7 h | derived below |

## What each result means

| Result | Reading |
|---|---|
| KEEP | With the writer fixed, the hook's explored context resolves more tasks than the native context of the same interleaved window: the WCC gain of enrich carries into patch quality. Method (a) measures context quality only, not agent behavior. |
| NOT CONFIRMED | No resolved difference at this n (MDE 6 to 8 points). It does not show "no effect": the IRP WCC gain (+0.012) can be real and too small to move resolved. |
| DROP | The hook's context lowers resolved: its files displace context that the writer needs. |
| VOID (writer placebo) | The writer alone moves resolved at alpha on the same context: the writer's noise is larger than the design assumes; no comparison of contexts can be read. |
| VOID (exploration placebo) | Two native explorations of the same window differ in resolved at alpha: one exploration session per task is too noisy for this comparison. |
| VOID (health) | A rule of the run failed; the reason is in result.json. |
| Replication or pooled test differs from the primary | Reported as a replicate disagreement; the verdict stays the primary's. |
| L1 c above 0.5, CI excludes 0.5 | Within a task, the context with the higher WCC more often gives the resolved patch: WCC is evidence about patch quality. |
| L1 CI includes 0.5 | WCC does not order contexts by their effect on a resolved patch at this n. |

## Stop rules

- The run: the exit codes of HEALTH and COST CAP above. A usage limit (75), or a memory dip below 24 GB available (3): wait, then run the same command again. Any other stop: main reads S/STOPPED and S/logs/run.log and decides; a resume skips every done step.
- The analysis: one run of analyze.py after the run ends with exit 0. No interim look at resolved by label. No change to analyze.py after the launch: run_full_a.sh checks F/SHA256SUMS before each run.

## Limits

1. The IRP enrich arm ran the Python prototype hook (hooks_product/prompt_map.py with the CLI 81d5a7e), not the product hook of 14f8a95. Its emitted text equals the product hook's on 297 of 300 prompts (PMH parity).
2. The writer is r = 1 per context except the writer placebo; the explorer replicates r1 and r2 give the replication.
3. The writer reads the v1 issue text that the sessions saw; Pro grades with V2. V2 rewrote the problem statement of 24 of the 56 H240 Pro tasks in V2 (29 unchanged, 3 escapes only; S/schedule_meta.json, limits). The effect is the same for every label of a task.
4. Verified images come from the Epoch registry (rebuilt from the SWE-bench build scripts), not the official Docker Hub images; stage 0 checks each task on its image.
5. One writer model, one effort, one Claude Code version; the WCC of each context is the stored score of its IRP explorer session.
6. H240 excludes flat60, the set where the PH2 screen found the signal; the population is the confirmation set of PH2C and IRP.

## Time (derived from the measured figures)

Grading is the bottleneck. Grading-slot time = row grades + stage 0: Verified 130 x (9 x 22.8 s + 2 x 23.1 s), Pro 56 x (9 x 29.2 s + 2 x 44.6 s), Multilingual 47 x (9 x 18.9 s + 2 x 18.5 s) = 62,100 s; over 4 slots = 4.3 h (the pilot's per-step means). A 10% to 50% slowdown from 4 graders at once gives 4.7 to 6.5 h. The two dry runs measured 3.4 and 6.4 grading-slot minutes per task, which gives 3.3 to 6.2 h for 233 tasks. Not on the path: writer calls 2,097 x 11.9 s / 4 = 1.7 h; checks 2,097 x 2.3 s / 2 = 0.7 h; pulls 231 x about 25 s = 1.6 h on one worker, 8 blocks ahead. Docker Hub pacing adds 0 h: 46 pulls stay below the 90 per 6 h share (the documented anonymous limit is 100 per 6 h; this address showed 100 per 1 h on 2026-11-06). Total: about 3.5 to 7 h, plus any wait for a usage-limit reset.

## The dry run (F/dry, bench 0ae1da3, fake claude, before the real run)

3 H240 tasks, the first of each dataset in block order: fmtlib__fmt-3729 (Multilingual, Docker Hub), protonmail__webclients-7b833df1 (Pro, V2; the pilot image was present), django__django-16612 (Verified, Epoch). 27 rows; stage 0 3/3 pass; 27 gen, 27 grade, 27 checks done; 0 failed attempts; cost $0.00; 5.2 min wall. Each row got a gold, no-op or empty fake patch (rotated by block), and each grade equals its planted kind. At most 4 writer and 4 grading steps at once. 2 pulls (0.57 GB in 6 s, 2.36 GB in 28 s); 3 removals, all in W/images.jsonl. Owner storage 8 containers and 23 images, same sorted-id sha256, before and after. analyze.py: health 6/6 ok, verdict NOT CONFIRMED (n = 3); the replication and the pooled sign test print. dry_proof.py PASSES (F/dry/proof.txt). A second invocation found every step done (exit 0, 0 new state rows). run_full_a.sh --check on S with the real claude: "every health check passes", exit 0.

## Launch

From the shell of main, after the commit of this plan and the registry line:

    /var/tmp/claude-rust-port/pqp/run_full_a.sh > /var/tmp/claude-rust-port/pqp/full-a/logs/run.out 2>&1

After a stop with exit 75 or 3 (a usage limit or a memory dip): the same command, appended with >>. After exit 0:

    <swe-explore-bench>/.venv/bin/python /var/tmp/claude-rust-port/pqp/full-a/analyze.py \
        --state /var/tmp/claude-rust-port/pqp/full-a/run

## Draft registry line (status planned)

F/registry-line.json: one JSON object in the 3.1 fields, appended to docs/analysis/runs/registry.jsonl in the commit after this plan, with this plan's sha256 and commit time. Its content:
- id 2026-11-06_qfa-method-a-h240;
- bench: quality-full-a at 0ae1da3, not dirty;
- builds: agentless-mcp b69b8eff, the V2 tasks at 66f9276, the image plan 7a70fef7;
- arms: the 9 labels with their frozen input sha256 and their roles;
- tasks: H240, n 240, with the sha256 above;
- schedule: 2c73ca6d..., seed 20261105, replicates 1;
- sessions: 2,160 planned;
- files, each with sha256 and bytes: the schedule, the meta, tasks, the reference files, the image plan, analyze.py, its test, SHA256SUMS, run_full_a.sh.
