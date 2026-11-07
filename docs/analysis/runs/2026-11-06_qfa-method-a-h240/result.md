# Result: method (a) on H240, the prompt-map hook's context against the native context

Registry id: `2026-11-06_qfa-method-a-h240` (runs/registry.jsonl). Plan: [plan.md](plan.md), sha256
cdec1371..., committed at 23:34:17 UTC on 2026-11-06, 37 seconds before the first run step and 82 seconds before
the first writer call. Analysis script sha256 8827e9e4..., run one time after the run ended with exit 0. Output
`result.json` sha256 0dd7f485..., `result.txt` sha256 4d5e9d6a....

## Verdict

NOT CONFIRMED. The pass rule finds no resolved difference between the hook's context and the native context. Both
placebo checks pass, and the 6 health rules pass, so the run is not VOID.

| Comparison (control irp_native_r1 unless named) | b | c | PD | D, 95% CI | exact p | Rule |
|---|---|---|---|---|---|---|
| primary: irp_enrich_r1 | 11 | 18 | 0.127 | -3.1 points [-7.9, +1.3] | 0.265 | NOT CONFIRMED |
| placebo (writer): writer_placebo | 13 | 14 | 0.118 | -0.4 points [-4.8, +3.9] | 1.000 | NOT CONFIRMED: passes |
| placebo (exploration): irp_native_placebo_r1 | 12 | 18 | 0.131 | -2.6 points [-7.4, +2.2] | 0.362 | NOT CONFIRMED: passes |
| replication: irp_enrich_r2 against irp_native_r2 | 15 | 13 | 0.122 | +0.9 points [-3.5, +5.2] | 0.851 | NOT CONFIRMED |

n = 229 tasks. b counts the tasks that the treatment resolves and the control does not, and c counts the reverse.
D = (b - c) / n, with a bootstrap 95% CI over tasks (seed 20261105, 10,000 resamples).

The pooled sign test (reported, not a rule): per task, the resolved count of irp_enrich_r1 and r2 minus that of
irp_native_r1 and r2. 18 tasks are positive, 23 negative and 188 zero. The distribution from -2 to +2 is 2, 21,
188, 16, 2. The mean is -0.022 per task, and the exact p is 0.533: NOT CONFIRMED.

## Resolved per label

| Label | Resolved | Share, Wilson 95% CI | apply_failed | unresolved | Verified (126) | Pro (56) | Multilingual (47) |
|---|---|---|---|---|---|---|---|
| irp_native_r1 | 176 | 0.769 [0.710, 0.818] | 12 | 41 | 94 | 38 | 44 |
| irp_native_r2 | 172 | 0.751 [0.691, 0.803] | 13 | 44 | 96 | 35 | 41 |
| irp_enrich_r1 | 169 | 0.738 [0.677, 0.791] | 17 | 43 | 95 | 32 | 42 |
| irp_enrich_r2 | 174 | 0.760 [0.701, 0.811] | 15 | 40 | 94 | 37 | 43 |
| irp_native_placebo_r1 | 170 | 0.742 [0.682, 0.795] | 16 | 43 | 93 | 33 | 44 |
| irp_native_placebo_r2 | 169 | 0.738 [0.677, 0.791] | 15 | 45 | 95 | 34 | 40 |
| writer_placebo | 175 | 0.764 [0.705, 0.815] | 11 | 43 | 99 | 37 | 39 |
| no_context | 91 | 0.397 [0.336, 0.462] | 90 | 48 | 71 | 11 | 9 |
| oracle | 177 | 0.773 [0.714, 0.822] | 14 | 38 | 98 | 42 | 37 |

Any comparison of labels inside one dataset is exploratory. The plan names no test for it.

## L1: does WCC order the contexts of a task by their effect?

c = 0.509 [0.413, 0.604] over 373 informative pairs in 59 tasks. The CI includes 0.5, and c is below the
predicted range of 0.55 to 0.70. Inside a task, the context with the higher weighted_core_coverage (WCC) gives the
resolved patch no more often than chance at this n. The conditional logistic fit agrees: odds ratio 0.969 per 0.1
WCC [0.681, 1.379] over 59 strata. The second fit (a non-test file of the reference fix in the context) is not
estimable: only 3 strata inform it, and the estimate diverges (separation).

## The deterministic checks (plan 4.1)

The checks find no difference between the explored labels. On each of the 7 explored labels (the 6 IRP contexts
and the writer placebo), the median patch changes 1 file and adds 5 to 6 lines. Per label, the checks report 35 to
44 complexity_delta warnings, 21 to 24 dangling_references warnings and 7 to 13 clone_delta advisories, and 17 to
22 patches change a non-test file outside the reference fix. 0 patches are empty. The not_checked counts show the language
coverage of each check: for example, contract_drift does not check 172 to 181 of the 229 patches per label. The
full counts per label are in `result.json` under `checks`.

## The cost gate (reported, not a rule)

| Label | Context tokens per call | Output tokens per call | Priced cold per call | USD |
|---|---|---|---|---|
| irp_native_r1 | 3,923 | 1,978 | 14,792 | 6.51 |
| irp_native_r2 | 3,938 | 2,086 | 15,350 | 8.03 |
| irp_enrich_r1 | 3,936 | 2,083 | 15,336 | 8.16 |
| irp_enrich_r2 | 3,790 | 2,173 | 15,601 | 8.12 |
| irp_native_placebo_r1 | 3,742 | 1,947 | 14,409 | 7.44 |
| irp_native_placebo_r2 | 3,842 | 2,388 | 16,740 | 8.42 |
| writer_placebo | 3,923 | 2,497 | 17,386 | 7.80 |
| no_context | 1,428 | 2,315 | 13,357 | 6.61 |
| oracle | 16,152 | 2,991 | 35,143 | 21.64 |

The total over all attempts is $82.73 API-equivalent (Claude Code's `total_cost_usd`; not a charge, because the
runs use the owner's Max plan). The cap was $120.

## What this result means

1. The hook's explored context does not raise the share of tasks that a fixed writer resolves. The primary CI is
   -7.9 to +1.3 points, and the replication CI is -3.5 to +5.2 points. A gain above +1.3 points on replicate 1, or
   above +5.2 points on replicate 2, is outside the interval. The result does not show "no effect": the plan's MDE
   at this n is 6 to 8 points.
2. Descriptive, not a planned test: the native context (176 resolved) and the oracle context (177 resolved) give
   this writer almost the same share. The no_context anchor (91 resolved) shows that context matters by about 37
   points. On H240 with this writer, native exploration already gives the writer about as much as the core
   ground-truth regions do, so a better explored context has little room to raise resolved.
3. L1 finds no link between WCC and resolved inside a task. The IRP verdict (KEEP: enrich WCC +0.0119) is a
   retrieval result. This run does not find that the WCC gain carries into resolved patches.
4. The IRP run also measured enrich at -5.9% context tokens per exploration session. This run does not test that
   result. With no resolved difference found, the hook's measured case rests on the exploration cost, not on patch
   quality.

## The predictions against the result

| Prediction (plan) | Measured | Outcome |
|---|---|---|
| n after stage 0: 229 (225 to 233) | 229 | held |
| resolved, irp_native_r1: 0.52 (0.40 to 0.65) | 0.769 | failed (above the range) |
| resolved, oracle: 0.62 (0.50 to 0.75) | 0.773 | failed (above the range) |
| resolved, no_context: 0.32 (0.20 to 0.45) | 0.397 | held |
| primary D: +0.5 points (-3 to +4) | -3.1 points | failed (just below the range) |
| primary PD: 0.14 (0.08 to 0.20) | 0.127 | held |
| writer placebo PD 0.10 (0.06 to 0.15), D within +-3 | 0.118, -0.4 | held |
| exploration placebo PD 0.12 (0.06 to 0.18), D within +-3 | 0.131, -2.6 | held |
| verdict: NOT CONFIRMED (P 0.83) | NOT CONFIRMED | held |
| replication: NOT CONFIRMED, D -3 to +4 | NOT CONFIRMED, +0.9 | held |
| pooled sign test: NOT CONFIRMED, nonzero in 20% to 30% of tasks | NOT CONFIRMED, nonzero in 41 of 229 (17.9%) | verdict held, nonzero share failed (below) |
| L1 c: 0.60 (0.55 to 0.70), CI excludes 0.5 with P 0.6 | 0.509 [0.413, 0.604] | failed (below the range) |
| L1 informative pairs: 500 (300 to 750) in 60 to 110 tasks | 373 in 59 tasks | pairs held, tasks just below |
| clogit OR per 0.1 WCC: 1.2 (1.0 to 1.6) | 0.969 [0.681, 1.379] | failed (below the range) |
| clogit OR, reference file in context: 2.0 (1.3 to 3.5) | not estimable (separation) | not measured |
| apply_failed share per explored label: 0.15 (0.08 to 0.22) | 0.048 to 0.074 | failed (below the range) |
| cost: $63 ($50 to $85) | $82.73 | held |
| context and output tokens per call: 4,000 and 1,700 (3,000 to 5,500 and 1,200 to 2,500) | 3,742 to 3,938 and 1,947 to 2,497 on the explored labels | held |
| stops: 0 health, 0 integrity, 0 to 2 usage limit | 3 stops by rule (6), all on one task's image; 1 usage limit | failed (the rule (6) stops) |
| failed attempts: 3 (0 to 12) | 6, all on one task | held |
| image pulls: 231 | 231 (587.5 GB) | held |
| wall time: 5 h (3.5 to 7 h) | about 4.6 h of run time in a 6 h 16 min window | held |

The resolved predictions failed because the writer resolved more than the pilot's 10 tasks and the SWE-Explore
restricted-context writers suggested. A higher base rate leaves less room for any context to raise resolved.

## Deviations from the plan

1. After stop 1 (23:50:07 UTC, block 21), main added `ignore_chown_errors = "true"` under
   `[storage.options.overlay]` to the PQP storage.conf. The image of
   internetarchive__openlibrary-00bec1e7 owns files by IDs above the 65,536 subordinate IDs of this user, and
   podman could not unpack it. The file is not pinned in schedule_meta. With the option, a file with an owner ID
   that the user namespace cannot map unpacks as owned by the container's root. The option applies to every image
   pulled after 23:50 UTC. The same image serves every label of a task, and stage 0 still gates each task on its
   image.
2. Main pulled the openlibrary-00bec1e7 image (62174ccc) by hand through the PQP wrapper to confirm the fix. The
   run's image step for it has 3 failed attempts and no done row. At stop 2 (23:57:02 UTC) the wrapper refused the
   remove step because W/images.jsonl did not list the image, and main appended its pull row with a note. At stop
   3 (23:58:13 UTC) the remove step still had 3 failed attempts, and the guard of full_a.py stops each resume on
   such a step. Main removed the image by hand through the wrapper, then appended the remove row to
   W/images.jsonl and a state row (remove, attempt 4, done, the detail says by hand). Main changed no gen, grade
   or checks row.

## Stops and resumes

| Stop | Time (UTC) | Exit | Cause | Action |
|---|---|---|---|---|
| 1 | 23:50:07 | 6 | the openlibrary-00bec1e7 image did not unpack (3 attempts) | deviation 1, resumed 23:51 |
| 2 | 23:57:02 | 6 | the wrapper refused the remove of an unlisted image (3 attempts) | deviation 2, resumed |
| 3 | 23:58:13 | 6 | the remove step kept its 3 failed attempts across resumes | deviation 2, resumed 00:04:03 |
| 4 | 03:19:38 | 75 | the writer reached the usage limit (gen:1665) | resumed 04:52:01 after the reset |

The usage-limit stop is the plan's rule. Its 2 writer attempts are recorded as usage_limit, not as failures, and
both ran again after the reset. The isolation proof passed before and after each segment: the owner's default
storage held 8 containers (sorted-id sha256 33d4c71a...) and 23 images (97740034...) every time.

## Health

| Rule | Detail | ok |
|---|---|---|
| complete | 0 included rows without a grade or a checks row | yes |
| stage 0 | 229 tasks in, 11 excluded, 0 failing but in | yes |
| grades | 2,527 grade rows: 0 errors, 0 outside 6 GiB and swap 0 | yes |
| writer | 2,061 writer calls, model set {claude-sonnet-5-5}, 0 bad envelopes | yes |
| pinned | 9 pinned state files checked, 0 changed | yes |
| finished | no STOPPED marker | yes |

Excluded tasks (11). No V2 grader (7): element-web-ca8b1b04, ansible-1a4644ff, ansible-b8025ac1, ansible-deb54e4c,
NodeBB-51d8f3b1, tutanota-1e516e98, tutanota-befce4b1. The gold patch does not resolve its own task in stage 0
(4): astropy-7606, astropy-8707, django-10097, matplotlib-20859. The peak memory of a grading container was
4.87 GiB.

## Limits

1. The IRP enrich arm ran the Python prototype hook (hooks_product/prompt_map.py with the CLI 81d5a7e), not the
   product hook of 14f8a95. Its emitted text equals the product hook's on 297 of 300 prompts (PMH parity).
2. Method (a) measures the quality of the explored context with a fixed writer. It does not measure how the hook
   changes an agent that explores and writes in one session. The pilot found method (b), which measures that, at
   ceiling.
3. One writer model (claude-sonnet-5-5, effort high), one Claude Code version (2.1.318), one task set. The writer
   is r = 1 per context except the writer placebo.
4. The writer reads the v1 issue text that the IRP sessions saw. Pro grades with V2, which rewrote the problem
   statement of 24 of the 56 Pro tasks. The effect is the same for every label of a task.
5. The oracle context is the core ground-truth regions under the 2,000-line cap. A wider oracle (for example with
   the tests or the callers) can give a higher ceiling than this one.
6. The result files exist only on the working disk. The archive is not set up.

## Reproduce

From the run directory `/var/tmp/claude-rust-port/pqp/full-a` (F), with S = F/run:

```bash
sha256sum -c SHA256SUMS        # analyze.py, test_analyze.py, image_plan.jsonl
PYTHONDONTWRITEBYTECODE=1 <bench>/.venv/bin/python analyze.py --state run   # writes run/analysis/result.json and result.txt
```

`<bench>` is the swe-explore-bench clone whose virtual environment ran the analysis. The script reads
S/schedule_meta.json, S/schedule.jsonl, S/tasks.jsonl, S/excluded.jsonl, S/state.jsonl, S/a/stage0.jsonl,
S/a/grades.jsonl, S/a/checks.jsonl, S/a/writer.jsonl, S/inputs/<label>.top10.jsonl and
S/inputs/reference_files.json.
