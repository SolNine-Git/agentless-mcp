# Declaration-role regression and rollback

**Status:** resolved by the 0.7.1 rollback; recorded 2026-08-28.

This note records why the cross-language declaration-role change was withdrawn,
what evidence supports that decision, and what a future attempt must prove. It
is a current guardrail, not an archived assessment. Read
[`benchmark-methodology.md`](benchmark-methodology.md) for the statistical and
run-integrity rules used below.

## Conclusion

Commit `5a061147` extended declaration-name marking from Python to languages
whose tree-sitter configuration names declaration forms. It also let a marked
declaration bypass the resolver's conservative same-line guard. The intent was
to stop treating declaration identifiers as references without dropping a
second, real reference on the same line.

The change was semantically plausible but unsafe. It changed deterministic map
output in 20 of the 60 pilot repositories, with 11 substantive changes, and the
agentic regression was concentrated in the later, non-Python-heavy part of the
pilot. The affected run repeatedly showed more noisy files and regions, fewer
hit files, and lower context efficiency than the pinned 0.7.0 arm. Cache lookup
latency was not involved.

The rollback restores the behaviour at clean cache-only commit `35ccc956`:

- declaration roles are marked only for Python;
- `declaration_name_ids` recognized class and function declarations only, and
  was deleted with the rest of the scaffolding in the post-review cleanup;
- the resolver drops any reference whose name and line coincide with a symbol
  declaration, as it did in 0.7.0; and
- the cache schema returns from 15 to 14 because the persisted role vocabulary
  no longer changed.

The rollback is the correct shipping decision. It removes a deterministic
cross-language output change that had no demonstrated quality benefit. The
agentic benchmark does not prove that every observed score loss was caused by
this code: after rollback, the primary paired deltas against 0.7.0 were
statistically indistinguishable from zero.

## What changed and why it mattered

The change crossed three coupled boundaries:

1. `core.extractor.collect_refs` began assigning
   `IdentifierRole.DECLARATION` to configured non-Python declaration names.
2. `marks_declarations` advertised that stronger role vocabulary for those
   languages, and `declaration_name_ids` expanded beyond class and function
   declarations to function-valued bindings and fields.
3. `core.resolve._reference_edges` allowed other occurrences sharing a marked
   declaration's name and line to survive the old same-line fallback.

That is not merely an internal representation change. Reference edges feed the
repository graph, ranking, focused maps, and the structural context rendered to
an agent. A one-token classification change can therefore reorganize which
files and symbols the agent sees even when most calls remain byte-identical.

The cache schema bump was a consequence, not the regression. Schema 15 was
needed only because cached non-Python rows contained the expanded role
vocabulary. Reverting the vocabulary requires schema 14 so the implementation
and persisted contract agree.

## Evidence

### Historical symptom

The recorded 59-instance comparison reported these 0.7.1-minus-0.7.0 point
deltas:

| Metric | Delta | Direction |
| --- | ---: | --- |
| hit-file rate | -0.0349 | worse |
| noise-file rate | +0.0370 | worse |
| hit-region rate | -0.0233 | worse |
| noise-region rate | +0.0289 | worse |
| weighted core coverage | -0.0155 | worse |
| context efficiency | -0.0277 | worse |

Those directions were operationally concerning and had recurred across runs,
but the paired-bootstrap 95% interval for each delta crossed zero. The run was
evidence to investigate a deterministic change, not proof of six independent
regressions. The metrics share files and lines and are strongly correlated.

### Deterministic attribution

Direct comparison of 0.7.0, clean cache-only, and the declaration-role commit
found:

- clean cache-only and 0.7.0 returned byte-identical normal tool responses in
  90 of 90 calls across 12 repositories;
- their `tools/list` schemas were identical;
- the declaration-role commit changed 20 of 60 deterministic maps, 11
  substantively; and
- the adverse agentic movement was concentrated in non-Python repositories,
  the only repositories whose extraction semantics had changed.

This identifies the declaration-role cluster as the only observed
product-output perturbation aligned with the symptom. It does not isolate one
line inside the coupled extractor/resolver change.

### Fresh rollback validation

The validation pinned three arms and ran them in parallel with Sonnet, one
worker per arm, a 900-second timeout, and the same 60-instance pilot:

| Arm | Commit or configuration |
| --- | --- |
| native search | Claude `Read`, `Glob`, and `Grep` |
| 0.7.0 | detached clean `d381eb94` |
| rollback | detached clean `35ccc956` |

All arms timed out on the same ProtonMail instance, leaving 59 matched valid
rows. The MCP preflight returned five tools for both structural arms;
`tools/list` evidence existed for every proof log; median MCP calls were four;
and the capabilities-call rate was zero.

Rollback-minus-0.7.0 results:

| Metric | Delta | Paired-bootstrap 95% CI |
| --- | ---: | ---: |
| precision | +0.0104 | [-0.0524, +0.0734] |
| recall | -0.0094 | [-0.0329, +0.0122] |
| F1 | -0.0089 | [-0.0382, +0.0196] |
| hit-file rate | +0.0095 | [-0.0303, +0.0480] |
| noise-file rate | -0.0045 | [-0.0483, +0.0408] |
| hit-region rate | -0.0056 | [-0.0522, +0.0378] |
| noise-region rate | +0.0051 | [-0.0302, +0.0408] |
| weighted core coverage | -0.0141 | [-0.0490, +0.0184] |
| context efficiency | -0.0078 | [-0.0621, +0.0477] |
| nDCG@100 | -0.0201 | [-0.0583, +0.0122] |

No primary interval excludes zero. The result supports "the large observed
regression is no longer present at this instrument's resolution," not
"rollback is better than 0.7.0." Rollback still had adverse point estimates on
most correlated recall and ranking columns.

The native-search comparison was also inconclusive. Rollback had nominally
higher F1 (+0.0134) and hit-file rate (+0.0305), but every primary interval
crossed zero.

## Noise floor and metric caveats

The fresh 0.7.0 arm moved materially from the prior recorded 0.7.0 arm despite
using the same commit, model, benchmark SHA, and rescored metric schema:

| Metric | Fresh minus prior 0.7.0 | Paired-bootstrap 95% CI |
| --- | ---: | ---: |
| hit-file rate | -0.0255 | [-0.0663, +0.0143] |
| noise-file rate | +0.0284 | [-0.0173, +0.0774] |
| hit-region rate | -0.0371 | [-0.0814, +0.0058] |
| weighted core coverage | -0.0266 | [-0.0611, +0.0079] |
| context efficiency | -0.0167 | [-0.0730, +0.0379] |

The historical arm used six workers while the fresh validation used one worker
per parallel arm. Treat these deltas as a provisional operational noise bound,
not a clean same-configuration replicate. They are nevertheless large enough
to show why aggregate point direction alone cannot promote or reject a change.

The strict non-bulk cohort exposed a separate instrumentation caveat. The
0.7.0 arm emitted three `end=-1` spans and requested 3,417 mean lines, while
rollback requested 196. `context_efficiency` still favoured 0.7.0 because it
credits optional ground-truth regions and is not a line-cost metric. Read it
beside mean predicted lines and the span-normalized recall/WCC columns; never
use it alone as proof of denser information.

One byte-level protocol difference remains: MCP `serverInfo.version` is
`0.7.0` in the control and `0.7.1` in rollback. Capabilities, tool schemas, and
normal responses otherwise match. Version metadata is an untested confound,
not an established agent-behaviour cause.

## Regression gates for any future attempt

A future cross-language declaration-role change must pass all of these gates:

1. **Per-language extraction characterization.** For every affected language,
   cover class names, function names, function-valued bindings, fields, and a
   real reference sharing a declaration line. Assert roles and resolved edges,
   not only rendered text.
2. **Deterministic output review.** Produce byte diffs for normal and focused
   maps, references, and explanations on the multilingual fixtures. Every
   changed edge or rendered line must be an intended semantic correction.
3. **No flattened-score shortcut.** Report both deterministic flattening modes
   or neither, as required by the benchmark methodology.
4. **Pinned agentic arms.** Compare native search, the shipped structural arm,
   and the proposed change in clean detached worktrees with identical MCP
   preflight and proof-log checks.
5. **Replicate before promotion.** Establish a same-arm, same-worker replicate
   noise floor. A primary quality claim requires a paired-bootstrap interval
   excluding zero in two treatment replicates and a magnitude larger than the
   observed same-arm movement.
6. **Information cost travels with quality.** Report mean predicted lines,
   EOF spans, whole-file emissions, span-normalized recall/WCC, output tokens,
   tool calls, turns, and cost. A recall gain bought by unbounded spans is not
   an improvement.

Stop the experiment if deterministic maps change without a per-edge correctness
argument, or if either treatment replicate fails the quality threshold. Do not
ship the broader role vocabulary on the theory that more precise syntax labels
must help ranking; this incident is the counterexample.

## Reproduction record

The fresh result directories in the sibling harness were:

```text
results/claude_code_native_20260828_r1
results/claude_code_agentless_hooked_main070_20260828_r1
results/claude_code_agentless_hooked_rollback_20260828_r1
```

The quality report was generated with:

```bash
uv run python score_report.py \
  --results results \
  --bench pilot_sample.jsonl \
  --repos . \
  --top-k 10 \
  --all \
  --arms \
    claude_code_native_20260828_r1 \
    claude_code_agentless_hooked_main070_20260828_r1 \
    claude_code_agentless_hooked_rollback_20260828_r1 \
  --paired-control claude_code_agentless_hooked_main070_20260828_r1 \
  --paired-treatment claude_code_agentless_hooked_rollback_20260828_r1
```

Correctness gates on the rollback checkout reported 279 focused tests passed,
3,103 total tests passed, 10 expected skips, 95% combined coverage, and every
pre-commit hook passing.
