# QMD design: the measurement standard and the code-quality experiment

Ticket QMD. Written 2026-11-04 (local) by the QMD agent. Design only: no session, no build, no container command.
Every number is "measured" (the script and its output file in W = /var/tmp/claude-rust-port/qmd are named) or
"derived" (the arithmetic is shown or named). Sources are in section 8. The docs MCP server did not answer (3
semantic_search calls and 1 web_search call each timed out at 1800 s while the server ran a docling batch). The
research used the fallback: WebSearch for leads, and curl for the page text (saved in W/sources).

## 1. The measurement standard

### 1.1 The analysis plan: one page, filled in and committed before the first session

```
RUN ID:            <yyyy-mm-dd>_<short-name>        REGISTRY ENTRY: runs/registry.jsonl line <n>
WRITTEN:           <UTC time> by <author>            PLAN SHA256: <sha256 of this file as committed>
QUESTION:          <one falsifiable sentence: "Does <treatment> raise <metric> against <control> on <set>?">
PRIMARY METRIC:    <one metric> (direction: higher is better | lower is better)
POPULATION:        <task-set id> (<n> tasks, file <path>, sha256 <...>); chosen by <script, seed>, never by outcome
ARMS:              control <label>: <build commit, artifact sha256, settings sha256>
                   treatment <label>: <the same, and the ONE difference from control in one line>
                   placebo <label>: a second copy of the control, same build and settings
REPLICATES:        r = <sessions per arm per task> on <all tasks | the subset <id>, n=<m>>
SAMPLE SIZE:       n = <tasks>. Source SD = <value> from <run id, script, output file>.
                   MDE = (z_alpha + 0.8416) x SD_r / sqrt(n) = <value> at power 0.80, alpha <value>, <1|2>-sided.
                   Comparisons k = <k>; correction <Bonferroni | Holm | none (k = 1)>.
ORDER:             schedule <path, sha256>, seed <int>: blocks = tasks in random order; arms in random order inside
                   each block; one window; <w> workers; failed rows are retried after the last block, at most 2 times.
PASS RULE:         KEEP when <exact inequality on the primary metric, with the CI rule>. DROP when <...>.
                   NOT CONFIRMED when <...>. Secondary metrics are reported. They never change the verdict.
PLACEBO RULE:      apply the PASS RULE to placebo - control. If the placebo passes, the run is VOID.
COST GATE:         <context tokens per session (input + cache write + cache read), and output tokens>.
                   The priced cost is reported with request 0 repriced cold (section 1.2, rule 4).
COST CAP:          <USD API-equivalent> and <sessions>. Stop the run when either cap is reached.
HEALTH (VOID IF):  <first-instance checks, rows per arm, 0 errors, tools/servers/style per init event,
                   proof-log tools/list rows, 0 relative-cache warnings, hook rows once per session>
PREDICTIONS:       <point and range for the primary metric, the cost gate, and each health count>
ANALYSIS SCRIPT:   <path, sha256>; run once on the dry-run output before the real run
EXPLORATORY:       <analyses that are not in this plan are labeled "exploratory" in every report>
```

The plan is the pre-registration: it states the question and the analysis before any outcome exists [S11]. For
method (a) the sessions exist, but no patch exists, so the outcome (resolved) is still unobserved.

### 1.2 Rules for every run

1. Interleave. Each task is one block. The arms of a block run in random order, one after another. The blocks run
   in random order. All arms run in one window. Reason: in the sequential PH2 screen, every arm after native gained
   +0.014 to +0.016 WCC, the placebo included, and in PH2C the 70 silent held-out sessions gained as much as the
   emitted ones (main, ph2c/analysis/order_check.py). Arm order and run time are a confound to remove.
2. Every run has a placebo arm. It is a second control, scheduled like any other arm.
3. Replicates: r = 2 on at least 60 tasks of every run, so each run measures its own session SD.
4. Cost. Decisions use context tokens and output tokens. These do not depend on the cache. The priced cost
   (input 1, cache write 1.25, cache read 0.1, output 5) depends on which session wrote the cache first. Report it
   "cold": reprice the cache-read tokens of request 0 at the write weight. Cross-session cache reads can occur only
   in request 0, because every later request extends the session's own prefix. Never compare raw USD.
5. Pin and freeze, as now: the bench worktree at a commit, the build by sha256, the Claude Code version, isolated
   settings (incident 4 rules), absolute XDG_CACHE_HOME and proof paths, DISABLE_AUTOUPDATER=1.
6. One primary metric decides. Report the other metrics with their CIs, labeled secondary.
7. Every run gets one registry entry (section 3) before its first session, status "planned".

### 1.3 What run_ab.py supports now, and the change the order needs

Now (read in run_ab.py at e7d0885): one arm per process; tasks in bench-file order (--instances filters, it does
not reorder); --resume skips tasks that have a top-k row; --limit, --tag, --workers, --allow-concurrent; one
run_manifest line per invocation; a lock per results directory; runlog rows without a start time, a position or a
replicate index. It has no schedule, no seed and no interleave of arms.

The change, on a branch of our bench clone (ph2s/bench), later, by the run's builder:
- make_schedule.py: reads only the task-set file and the arm list; writes schedule.jsonl (index, block, task, arm
  label, run_ab arm, replicate, env profile) from a seed. Replicate 2 is a separate label (suffix _r2).
- run_schedule.py: checks the schedule sha256 against the plan. For each row it runs
  `run_ab.py --arm <arm> --tag <label> --instances <task> --resume` with that arm's environment and the
  systemd-run scope that run.sh builds today. It appends schedule_log.jsonl (index, label, task, start and end in
  UTC, exit code, retry count). It stops on a failed health check.
- run_ab.py needs no change. Optional: a started_at field in the runlog row. Cost: about 1 to 2 s of process start
  per row against about 10.5 s per exploration session (measured mean wall, out/runcost.txt).

## 2. The minimal viable screen (main's added deliverable)

### 2.1 The task sets

| Set | Size | Content | Use |
|---|---|---|---|
| S100 | 100 | stratified draw from bench300, seed 20261105, largest-remainder allocation by dataset x language: Verified python 54; Pro python 10, go 8, js 6, ts 1; Multilingual c 3, go 2, java 4, js 2, php 3, ruby 3, rust 4 | stage 1 screens, fixed |
| H200 | 200 | bench300 minus S100 | stage 2 confirmation |
| F548 | 548 | the bench tasks outside bench300; no snapshot on disk yet | fresh confirmation sets later |

The draw script reads only instance_id, dataset and language (lang_map.json). It never reads a result. The
allocation above is measured (python3 inline, from bench300.jsonl and lang_map.json). Retire H200 to screening
after 3 confirmations. Then draw a fresh confirmation set from F548 (fetch_repos.py, then git init on each snapshot,
as incident 1 requires).

### 2.2 The funnel

| Stage | What runs | Decision rule | Detects (80% power) |
|---|---|---|---|
| 0, free | build and static gates; loc-bench byte diff when the ranking changes; the surface diff; a dry run of the hook over all 848 issue texts (emission rate and injected tokens per task, no model); the fakebin dry run of the full schedule | a variant that injects the same text as another is a duplicate: do not screen it; a variant that is silent on a share s of the tasks has its effect diluted by (1 - s), so the plan sizes n for the diluted effect | exact |
| 1, screen | S100; k variants + one shared native control + one placebo; interleaved; r = 2 | promote a variant when its paired WCC delta is above 0 at one-sided alpha 0.10/k (Bonferroni; familywise false promotion 0.10); VOID if the placebo passes | see table 2.3 |
| 1, early stop | after 50 blocks (all arms done) | drop a variant whose mean paired delta is 0 or less (futility only) | power loss 2.4% at the MDE (measured, out/power.txt) |
| 2, confirm | each promoted variant on H200, with native and placebo; interleaved; r = 2 | KEEP when the two-sided 95% CI (alpha 0.05/m for m variants) excludes 0 and the placebo CI includes 0 | 0.011 to 0.012 WCC |
| 3, code | method (a) on the stage 2 sessions of each KEEP variant | McNemar on resolved, plus the link (section 4.4) | 8.8 points at n=200, PD 0.20 |

### 2.3 What each size detects (measured, out/power.txt; MDE = (z_alpha + 0.8416) x SD / sqrt(n))

Inputs (measured, out/mvt_noise.txt, a re-run of main's mvt/noise.py): SD of the per-task paired WCC difference
0.0857 (placebo, one window, n=60), 0.0756 (placebo, two windows, n=291), 0.0675 (PH2C H enrich - native, n=240,
out/runcost.txt), treatment pairs up to 0.1054. For r = 2: SD_r = sqrt(sigma_int^2 + 2 sigma_s^2 / r), with
sigma_s^2 = placebo SD^2 / 2. With no task-by-arm interaction, r = 2 divides the SD by sqrt 2 (0.0857 -> 0.0606).

| Screen (one-sided, familywise 0.10) | k=1 | k=2 | k=3 | k=4 | k=6 |
|---|---|---|---|---|---|
| S100, r=1 (SD 0.0857) | 0.018 | 0.021 | 0.023 | 0.024 | 0.026 |
| S100, r=2 (SD 0.0606) | 0.013 | 0.015 | 0.016 | 0.017 | 0.018 |
| S150, r=2 (SD 0.0606) | 0.010 | 0.012 | 0.013 | 0.014 | 0.015 |

A task-by-arm interaction raises SD_r. With sigma_int^2 = 0.004 (derived: treatment SD^2 minus placebo SD^2 is
0.0034 for conf2 hooked and 0.0054 for conf2 forced; about 0 for enrich), SD_r at r = 2
is 0.083 to 0.088 and the S100 MDE at k = 4 is about 0.024. Recommendation: S100, r = 2, k at most 3. That screen
detects about 0.016 WCC. It costs (k + 2) x 100 x 2 sessions: 1,000 sessions at k = 3, about $30 and 3.5 h
(derived: $0.03 and 12.5 s per session, out/runcost.txt). An effect near 0.012 needs S150 or stage 2.

### 2.4 The first run under the standard: the interleaved rerun of PH2C

Arms: native, enrich, native_placebo. Tasks: H240 (the 240 held-out tasks of PH2C). One window, interleaved.
Measured inputs: observed effect +0.0123; H paired SD 0.0675 (out/runcost.txt).

| Design | Sessions | MDE (two-sided 0.05) | Power at +0.0123 | Power at +0.006 | Cost, time (derived) |
|---|---|---|---|---|---|
| r = 1 | 720 | 0.0122 to 0.0137 | 0.71 to 0.81 | 0.23 to 0.28 | about $22, 2.5 h |
| r = 2 (recommended) | 1,440 | 0.0086 to 0.0097 | 0.95 to 0.98 | 0.41 to 0.50 | about $44, 5 h |

The ranges use SD 0.0675 (H) and 0.0756 (two-window placebo). Cost: 0.0274 to 0.0329 USD per session as Claude Code
reports it (PH2C enrich and native means, out/runcost.txt); the priced tokens, 11,935 to 13,648 per session, give
$0.024 to $0.027 at $2 per million (Sonnet 5.5 [S9]). Time: 10.4 to 10.7 s per session plus about 2 s per row.

Pass rule to pre-register: KEEP enrich when (1) enrich - native WCC (per-task mean over replicates) has a two-sided
95% CI above 0; (2) native_placebo - native has a CI that includes 0, else VOID; (3) context delta at most +10%;
(4) the health checks of PH2C. Recorded, not gates: the delta on the tasks where the hook is silent in every enrich
session (prediction: CI includes 0); emitted minus silent; the position effect (first against third session of a
block; MDE 0.0145, out/power.txt).

## 3. The run registry and the archive

### 3.1 One index entry per run (one JSON object per line)

```
id, title, status (planned | running | complete | void), void_reason, question
plan            {path, sha256, committed_at}
dates           {window_start, window_end}             (UTC)
bench           {repo, branch, commit, worktree, dirty, tracked_diff_sha256}
builds          [{name, commit, artifact, sha256}]      (agentless binary, hook binary, bench-server project)
claude_code     {version, from}                         (from = the init event of the streams)
model           {alias, resolved}                       (resolved = the init event "model" field)
arms            [{label, run_ab_arm, settings_path, settings_sha256, env_profile, role}]
                                                        (role = control | treatment | placebo)
tasks           {set_id, file, sha256, n, instance_ids_sha256}
schedule        {path, sha256, seed, replicates, workers}
sessions        {planned, scored, failed, retried}
files           [{path, sha256, bytes, rows}]           (every top-k, runlog, run_manifest, schedule_log,
                                                         proof, gate log, analysis output)
transcripts     {dir, files, bytes, tree_sha256}        (tree_sha256 = sha256 of the sorted "sha256  path" list)
excluded        [{path, bytes, reason}]                 (.xdg-cache: derived, rebuilt from the build and snapshots)
verdict         {rule_results, text, analysis_output_sha256}
archive         {location, tarball, sha256, bytes, archived_at, verified_at}
```

Location: docs/analysis/runs/registry.jsonl in the agentless-mcp repository, with each plan at
docs/analysis/runs/<id>/plan.md. The index is small text, so review and git history apply to it. The result files
stay out of git. The registry entry names them by path and sha256.

### 3.2 Archive options (the owner decides)

Payload (measured, du with --exclude=.xdg-cache): conf2 72 MB, C8a about 17 MB, PH2 screen 15 MB, PH2C 25 MB:
about 130 MB in all, plus the frozen builds (C8a bin 242 MB, PH2 and PH2C bin 69 MB each). The caches are 29 GB
(conf2), about 6 GB (C8a) and about 3 GB (PH2 screen). Do not archive them.

| Option | For | Against |
|---|---|---|
| A. restic repository on an external disk | owner has physical control; no account; dedup and sha256 checks | one site; the disk must be connected |
| B. restic to encrypted cloud object storage | off site; encrypted before upload | an account and a recurring charge |
| C. private Hugging Face dataset repository | versioned; the datasets library reads it | the provider holds the data unencrypted |
| D. private GitHub repository, one release asset per run | simple; assets up to 2 GiB | not for large future runs |

Recommendation: A and B together (Q6). Each tarball holds one run; the registry entry records its sha256 and a
verified_at date after a test restore.

### 3.3 Backfill of the existing runs

| Run | Paths | Payload | Plan source | Known gaps |
|---|---|---|---|---|
| conf2 (2026-10-29) | bench results/*_conf2, transcripts/*_conf2 | 72 MB (+29 GB cache) | none found in the run directories | harness dirty (30 untracked entries, tracked diff empty); arms ran concurrently; Claude Code 2.1.312 and claude-sonnet-5-5 from the init events (measured, grep of 600 streams) |
| C8a screen | c8a/screen/results, transcripts, logs, keep-rule.md | about 17 MB (+6 GB cache) | c8a/screen/keep-rule.md | placebo base1/base2; 60 tasks |
| PH2 screen | ph2s/results, transcripts, logs, keep-rule.md | 15 MB (+3 GB cache) | ph2s/keep-rule.md | arms in sequence (order confound) |
| PH2C | ph2c/results, transcripts, logs, keep-rule.md, analysis | 25 MB | ph2c/keep-rule.md | arms in sequence (order confound) |

The backfill script (later) reads each run_manifest.jsonl, the init event of each stream, and the keep rule; hashes
every file; writes one entry per run with status "complete" and the gaps above in void_reason or a notes field.

## 4. The code-quality experiment

### 4.1 Outcomes (both methods, defined before any patch exists)

- Primary: resolved. Every FAIL_TO_PASS test passes and every PASS_TO_PASS test still passes, graded in a fresh
  container from the pristine image. Empty patch, failed apply and timeout count as not resolved.
- Deterministic checks on each patch, with the pinned agentless CLI (lint exists with these checks: measured, the
  `lint --help` text of the 2e87b0a wheel): `lint --diff <patch> --repo <base snapshot> --json` for
  undeclared_imports, dangling_references, dangling_callers, arity, shadowing, near_duplicates, cycle_delta; then,
  on a scratch copy with the patch applied, `lint --since <base> --only
  complexity_delta,clone_delta,callee_overlap,contract_drift --json`. Report findings per check and severity, and
  not_checked rows (language coverage).
- Patch size (lines added and removed, files changed); files changed that the reference fix does not change (test
  files excluded); for (b) turns, wall time, context tokens and the cold priced cost (section 1.2, rule 4).

### 4.2 Method (a): a fixed patch writer reads each arm's explored context

The protocol follows the SWE-Explore restricted-context validation [S1, appendix D]: the same patcher, prompt,
tools and budget for every arm; the explorer's regions are the only variable; the source benchmark's harness grades.

- Contexts (8 per task, 300 tasks): the top-10 regions of conf2 native, ungated, forced, hooked and of PH2C native,
  enrich (inputs given in the brief); anchor arms no_context (issue only) and oracle (the core ground-truth regions).
  Measured size of an explored context: mean 1,247 to 1,569 tokens (chars/4), median 891 to 1,103, about 100 to 170
  lines; at most 2 tasks per arm reach a 2,000-line cap (out/ctxsize.txt). Issue text: mean 416 tokens.
- Placebos: exploration placebo = conf2 native against PH2C native (two native sessions, 6 days apart, Claude Code
  2.1.312 and 2.1.318). Writer
  placebo = the PH2C native context written a second time (300 calls). Both measure the discordant share PD.
- Writer: one Messages API call per (context, task); claude-sonnet-5-5, adaptive thinking, one fixed effort, the
  Batches API (Q1). Temperature cannot be set on this model [S9], so the writer placebo is required.
- Reuse from quality/: the prompt template and _format_context_block of gen_patches.py, _extract_diff and
  _fix_patch_format; region reading as in build_context.py. Required changes: (1) delete the "Files that need to be
  modified" section: gen_patches.py and gen_patches_agent.py print the gold modified_core_files to the writer, a leak
  that gives every arm the answer; (2) a mode "explored" that reads regions from top10.jsonl; (3) the issue text from
  issue_map.json, the text the sessions saw; (4) the Anthropic SDK in place of the OpenAI-compatible client.
- Grading per dataset: Verified (161) and Multilingual (64) with the swebench 4.1.0 harness that the bench venv
  holds (it has the Multilingual specs, for example apache/druid: measured, harness/constants/java.py). Call
  run_swebench.py with --dataset SWE-bench/SWE-bench_Multilingual for those 64: its --auto-split has no
  Multilingual group and puts such tasks in "unknown", which it skips. Pro (75):
  run_swebench_pro.py runs pytest only, so it cannot grade the 44 go, js and ts Pro tasks. Grade Pro with the V2
  verifier (67 of the 75 tasks are in V2: measured, GitHub API listing of v2/tasks) (Q4).
- Stage 0 for grading: run the gold patch and an empty patch of every task through our grading path. The gold patch
  must resolve and the empty patch must not. A task that fails this gate leaves the analysis, with its reason logged.

### 4.3 Method (b): Claude Code repairs the issue, with and without the agentless tools

Design B, the agent on the host and the commands in the container:
1. For each session, start a new task container from the pristine image with --network=none, inside a systemd-run
   scope (section 5.3). Copy /testbed out of a temporary container to a host directory; start the run container
   with that directory bind-mounted at /testbed (--userns=keep-id). The image's installed environment still
   points at /testbed, so the tests run.
2. In the host copy, delete .git and commit the tree as one base commit. The history of an image can hold commits
   after the base commit; the agent must not read the fix from history. Do the same in every arm.
3. Run claude -p on the host with cwd = the host copy, the incident-4 isolation (--setting-sources project,local,
   --strict-mcp-config, CLAUDE_CODE_DISABLE_CLAUDE_MDS=1, --tools set per arm; WebFetch and WebSearch never in the
   list). A PreToolUse hook on Bash in every arm rewrites each command to run in the container
   (`podman exec -w /testbed <ctr> bash -lc <cmd>`). Read, Edit, Write, Grep, Glob and the agentless tools act on
   the host copy, which is the same tree the container sees.
4. The agentless MCP server and the gate hooks run on the host, as in the exploration bench, from the pinned
   build. They cannot run in the container: the binary needs glibc 2.39 (manylinux_2_39, measured: the wheel tag),
   the swebench Python images build on ubuntu:22.04 with glibc 2.35 (measured, harness/constants/__init__.py), and
   the claude binary is a dynamically linked glibc executable (measured, file).
5. At the end, take `git diff` of the host copy against its base commit. Grade it in a fresh container from the
   pristine image (never the agent's container), with the official grader of the dataset (section 4.2).

The arms stay equal except for the treatment: the same task prompt, model, Claude Code version, effort, turn cap and
a 50-minute wall cap (the V2 locked protocol uses 50 minutes [S3]), the same Bash hook, the same container, the same
start time (one block), and arm order random inside the block. Arms: native, native_placebo, and one agentless
configuration (Q5). Exploration score of a (b) session: WCC (the bench's evaluator) of the regions that the session
displayed before its first Edit or Write (Read offsets and limits from the stream; agentless reads from the proof
log).

### 4.4 The link between exploration and a resolved patch (stated before any data)

- L1 (both methods, primary for (a)): within-task concordance c. For each task and each pair of contexts (or
  sessions) with a different resolved outcome and a different WCC, score 1 when the resolved one has the higher WCC.
  In (a), L1 uses the 6 explored contexts only; the no_context and oracle anchors and the writer placebo stay out.
  c is the mean over pairs; the 95% CI comes from a bootstrap over tasks (10,000 resamples, seed 20261105). H0: c =
  0.5. Prediction: c between 0.55 and 0.70. Secondary: conditional logistic regression of resolved on WCC with task
  strata (odds ratio per 0.1 WCC), and the same with "a file of the reference fix is in the context".
- L2 ((b), stratified): group the tasks by the sign of the paired WCC delta (treatment - native: up, equal, down) and
  report the resolved delta with its CI in each group. Prediction, if understanding drives quality: the resolved
  gain is larger in "up" than in "down". WCC is not randomized, so L2 is evidence about the mechanism, not proof.
- Primary for (b): resolved, treatment against native, exact McNemar, two-sided 0.05. The placebo rule of 1.1
  applies.

### 4.5 Power for resolved (measured, out/power.txt)

Paired McNemar, two-sided 0.05, power 0.80, normal formula of Machin et al. (1997) [S7], equal to Connor's form
n = (z_a sqrt(PD) + z_b sqrt(PD - D^2))^2 / D^2, where PD = discordant share and D = difference in resolved share.

| n | PD 0.10 | PD 0.15 | PD 0.20 | PD 0.30 |
|---|---|---|---|---|
| 100 | 8.8 | 10.7 | 12.4 | 15.2 |
| 200 | 6.2 | 7.6 | 8.8 | 10.8 |
| 300 | 5.1 | 6.2 | 7.2 | 8.8 |
| 600 | 3.6 | 4.4 | 5.1 | 6.3 |
| 848 | 3.0 | 3.7 | 4.3 | 5.3 |

Main's figures check: 7.2 points at 300 and 5.1 points at 600, at PD 0.20. The exact conditional McNemar has
power 0.77 at those D (measured by enumeration), so the exact test needs about 5% more tasks. Assumptions: tasks
independent; PD is not measured yet; the pilot and the placebo arms measure it. With the public resolved rates near
0.67 to 0.71 for Sonnet-class agents [S8], a PD of 0.15 to 0.25 is plausible. A D of 3 points needs 1,742 tasks at
PD 0.20, more than the bench holds. So (b) on bench300 detects only a large effect, 6 to 9 points.

### 4.6 Cost and time (all derived; the pilot measures them)

| Item | (a) | (b) on bench300, 3 arms |
|---|---|---|
| Model calls | 2,700 (8 contexts x 300 + 300 writer placebo) | 900 sessions |
| Tokens per unit | input about 2,400 (500 prompt + 416 issue + 1,430 context, measured parts); output 2,000 to 6,000 (assumed) | public: 42 to 51 model calls per task for mini-SWE-agent with Sonnet 4.5 [S8] |
| USD per unit | $0.025 to $0.065 at $2 / $10 per million [S9]; half with the Batches API | $0.37 to $0.44 (public $0.56 to $0.66 at Sonnet 4.5 prices [S8], times 2/3 for Sonnet 5.5 prices); range $0.35 to $0.80 with the Claude Code prefix |
| USD total | $68 to $176 ($34 to $88 batched); cap $200 | $315 to $720; cap $800 |
| Grading runs | about 2,000 after removing empty and duplicate patches | 900 |
| Wall time | grading at 2 to 5 min per run (assumed), 4 containers: about 17 to 42 h | sessions at 3 to 10 min (assumed), 4 workers: 11 to 38 h; grading 8 to 19 h |
| Images | 300 (161 + 75 + 64) | the same 300 |

Pilot (for the owner's approval): 10 tasks from S100, drawn by seed: 5 Verified, 3 Pro (python, go, js), 2
Multilingual (java, rust). Steps: stage 0 grading gate (gold and empty patch, 20 grading runs); (a) 9 calls per
task (8 contexts + writer placebo) = 90 calls; (b) native, native_placebo and the agentless arm = 30 sessions. Cap:
$40 API-equivalent in all (derived need: about $6 for (a) and $11 to $24 for (b)). Stop at the cap, at the first
failed health check, or at 3 failed attempts of one step. The pilot measures tokens per call, session minutes,
grading minutes, image disk, peak memory per container and PD on the 10 tasks.

## 5. Feasibility on this machine (read-only checks, 2026-11-04 21:40 to 22:30 local)

### 5.1 Measured state

- Disk: / (which holds /var/tmp and ~/.local/share/containers/storage) 1.9 TB, 880 GB free (df -h).
- Memory: 123 GB total, 44 to 45 GB used, 78 GB available, swap 7 GB with 4 GB used (free -g). The GPU fleet held
  about 105 GB on 2026-10-10 (memory record), so the available figure moves.
- CPU: 32 (nproc). cgroup v2 user delegation: cpu io memory pids (the user@1000 cgroup.controllers file).
- Runtime: podman-5.8.9-1.fc43, toolbox-0.3-1.fc43; docker, podman-docker, moby-engine and docker-ce are not
  installed (rpm -q). The user podman.socket is inactive and disabled (systemctl --user, read only). The toolbox
  migrate marker (~/.config/toolbox/podman-system-migrate) reads 5.8.9. The running user services include the
  containers that a podman migrate stops (memory record): mcp-postgres, mcp-pgadmin, mcp-inference, llama-embed,
  llama-rerank, llama-fim, whisper-stt, kokoro-tts.

### 5.2 Images

- Counts: bench300 needs 300 instance images (161 Verified, 75 Pro, 64 Multilingual); the whole bench needs 848
  (451, 215, 182). All 848 tasks join to their dataset rows; each Pro and Multilingual task has its own image
  (measured, scripts/datasets.py on the cached Hugging Face parquet files).
- Compressed size per image (measured, Docker Hub tag metadata, 12 seeded tasks per dataset, no pull,
  out/hubsizes.txt): Verified median 1.06 GiB (mean 1.26), Pro median 0.90 (mean 1.52), Multilingual median 0.33
  (mean 0.40). Upper bound for bench300 without shared layers (derived): 343 GiB compressed; uncompressed is about
  2.3x compressed [S5], so up to about 790 GiB. Shared layers cut this: the 500 original Verified images hold 189 GiB
  of unique layers, and the optimized Epoch registry holds them in 30 GiB [S4]; LogicStar measured 240 GiB
  uncompressed in Podman and 31 GiB optimized [S5].
- Pull limits: Docker Hub allows 100 pulls per 6 hours without login and 200 with a personal login [S6]. 300 pulls
  take at least 18 h without login. Pro V2 images come from GHCR without login [S3]. Verified can come from the Epoch
  GHCR registry [S4]; the gold-patch gate (4.2) checks those rebuilt images.
- The SWE-bench docs ask for 120 GB free, 16 GB RAM, 8 cores, and fewer than min(0.75 x cores, 24) workers [S2].

### 5.3 Resource rules for the run

1. One container per systemd-run --user --scope with MemoryMax=6G and MemorySwapMax=0; podman --memory 6g too.
2. At most 4 containers at once. Start one only when free -g shows at least 24 GB available (the c8a/memwait.sh
   pattern, with a 24 GB floor). One builder at a time; nothing on /tmp (tmpfs).
3. Pull, grade and remove by batch. Keep at most 150 GB of images. Stop the run when / has less than 150 GB free.
4. Remove only the images that the run pulled, by the ID list in the run's image ledger. Never run a prune, a
   `podman system` command, or any toolbox command. Before the first podman command, compare `rpm -q podman` with the
   toolbox marker (memory record of 2026-11-02).
5. Keep the experiment's containers away from the docs stack: a dedicated Unix user with its own rootless podman
   (recommended, Q3), or at least a separate storage root and API socket (CONTAINERS_STORAGE_CONF under /var/tmp).
   The swebench harness and run_swebench_pro.py use the docker Python SDK, so they need DOCKER_HOST set to a podman
   API socket; that socket is off today (Q3).
6. The agent container has no network. The grading container keeps network (some Go tasks fetch modules [S3]).

## 6. Owner questions

| # | Question | Recommended answer | Reason |
|---|---|---|---|
| Q1 | Which patch writer for (a)? | claude-sonnet-5-5, one call per context, Batches API | the context is the only variable; no container in generation; cheapest ($34 to $88); an agentic writer adds tool noise |
| Q2 | Which billing for the paid steps? | an API key in a workspace with a spend limit, for (a) and (b) | a hard cost cap; the subscription limit failed 277 instances in one earlier run |
| Q3 | How do the experiment's containers run? | a dedicated Unix user with its own rootless podman and API socket (one sudo step by the owner) | a prune, a full disk or an OOM in the experiment cannot stop the docs stack |
| Q4 | Which SWE-bench Pro grader? | V2 verifier and images; the 8 bench300 Pro tasks not in V2 leave the resolved analysis; (a) keeps the v1 issue text that the sessions saw | V2 repaired 214 test patches and the verifier; run_swebench_pro.py cannot grade go, js or ts |
| Q5 | Which agentless arm and scope for (b)? | bench300; arms native, native_placebo, and enrich if the interleaved rerun keeps it, else the shipping hooked gate | 300 images and $315 to $720 first; 848 tasks only as a pre-registered second stage |
| Q6 | Where does the archive live? | restic on an external disk plus one encrypted cloud copy; the index in docs/analysis/runs/ | 130 MB today; two sites; the index stays reviewable in git |

## 7. Limits of this design

- PD, session minutes, grading minutes and output tokens per patch are assumptions until the pilot measures them.
- The image sizes come from 12 tasks per dataset and Docker Hub metadata; GHCR sizes were not read.
- The screen SDs come from 3 runs. A new treatment can have a larger task-by-arm interaction (table 2.3 note).
- Main's 2026-11-05 record says a WCC effect below about 0.02 is not measurable at 300 tasks. With the same formula
  and SDs, out/power.txt gives an MDE of 0.011 to 0.015 at 300 tasks (r = 1). Main can reconcile the two figures.

## 8. Sources

- [S1] Zhang et al., "SWE-Explore: Benchmarking How Coding Agents Explore Repositories", arXiv:2606.07297, HTML
  version: §3.4, §4.2, tables 3 and 4, appendix D (sources/sweexplore.txt). Downstream check: n=150, K=5,
  explorer-level correlations, Claude Code 48.0% resolved, Oracle 59.7%.
- [S2] SWE-bench repository README and docs/guides/docker_setup.md, main branch (sources/swebench_*.md).
- [S3] SWE-bench Pro repository README and v2/README.md; v2/tasks listing by the GitHub API (sources/swepro_*).
- [S4] Epoch AI, "swebench-docker", epoch.ai/blog/swebench-docker (sources/epoch_swebench_docker.txt).
- [S5] LogicStar, "How we made SWE-bench 50x smaller", logicstar.ai/blog/how-we-made-swe-bench-50x-smaller.
- [S6] Docker documentation, "Docker Hub usage and limits", docs.docker.com/docker-hub/usage/.
- [S7] NCSS PASS, "Tests for Two Correlated Proportions (McNemar Test)" (Machin, Campbell, Fayers and Pinol 1997;
  Schork and Williams 1980) (sources/pass_mcnemar.txt).
- [S8] SWE-bench leaderboards data, github.com/SWE-bench/swe-bench.github.io data/leaderboards.json: mini-SWE-agent
  with Claude Sonnet 4.5, Verified 70.6% at $0.558 per task and 51.0 calls (2025-09-29), 71.4% at $0.658 (2026-02-17);
  Multilingual 67.0% at $0.670 and 42.5 calls.
- [S9] Anthropic model prices from the claude-api skill (table cached 2026-10-06): Sonnet 5.5 $2 input, $10 output,
  $0.20 cache read per million; temperature removed on Sonnet 5.5.
- [S10] Local measurements, W/out: mvt_noise.txt, mvt_inert.txt, runcost.txt, ctxsize.txt, power.txt,
  datasets.txt, hubsizes.txt; scripts in W/scripts.
- [S11] Nosek, Ebersole, DeHaven and Mellor, "The preregistration revolution", PNAS 115(11), 2018, PMC5856500.

## 9. Owner answers (2026-11-05)

The owner accepted every recommended answer of section 6 and approved the first run of section 2.4 with two
sessions per task (r = 2, about $44, derived).

1. The patch writer for method (a): claude-sonnet-5-5. Correction (owner, 2026-11-05): the owner has a Max plan
   and no API key, so method (a) makes one `claude -p` call per context with every tool turned off, not a
   Batches API call.
2. Billing: every run uses `claude -p` on the owner's Max plan. The USD figures in this plan are Claude Code's
   API-equivalent estimates (`total_cost_usd`), not charges; the real limit is the plan's usage limits. A run
   that hits a usage limit stops, and resumes after the reset with only the failed tasks.
3. Container isolation: a dedicated Unix user with its own rootless podman and API socket (one sudo step by the
   owner).
4. The Pro grader: SWE-bench Pro V2. The tasks that are not in V2 leave the analysis.
5. The scope of (b): bench300 with native, native_placebo, and enrich if the interleaved rerun keeps it, else
   hooked.
6. The archive: restic to an external disk plus an encrypted cloud copy, with the index in docs/analysis/runs/.

Steps 2 to 4 of the plan (the pilot, method (a), method (b)) wait for the owner's setup of answers 2 and 3.
