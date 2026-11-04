# callee_overlap: the measurement record

This record holds the measurements of `lint`'s `callee_overlap` check, in the order of the
measurement plan in [the specification](../plans/2026-11-02_callee-overlap-duplicate-check.md)
(section 5). It holds M1, the cost, M2 (history), M3 (merged pull requests) and M4 (the
whole-repository sample). The false-positive rate stopped the measurement at its first check (stop
rule S1): bar A cannot pass, so the check stays opt-in and advisory. Bar B is not reached. The
readers then labeled the other 40 findings to test a confidence ranking (last section).

## M1: planted pairs

Each language has one small repository under `tests/characterization/fixtures/`:
`callee_overlap_rust` and `callee_overlap_py`. `base/` is the committed tree, `change/` lies over
it, and `pairs.json` names each planted pair. The test
`lint::tests::the_*_planted_pairs_give_the_golden_rows_and_no_row_on_a_negative` runs
`lint --since HEAD --only callee_overlap` over each one, compares the report with
`tests/characterization/goldens/lint/<name>.lint.json`, and asserts that every row is a planted
positive and that no row names a planted negative.

| Language | renamed | edited | re-implemented | in one file | across files | negatives with a row |
|---|---|---|---|---|---|---|
| Rust | 3/3 | 3/3 | 4/4 | 3/3 | 7/7 | 0/8 |
| Python | 3/3 | 3/3 | 4/4 | 3/3 | 7/7 | 0/8 |

The negatives: a sibling pipeline that shares 2 of 3 callees, a member of a family of 6 builders, a
protocol method (Rust `Display::fmt`, Python `__str__`), a test function (Rust `#[test]` in a
`cfg(test)` module; Python under `tests/`), a function moved to a new file, a function renamed in
place, a function that calls its original, and an original with 2 callees. Each negative is built
so that only its own filter keeps it out. That is derived from the fixture; no run removed a filter.

The planted helpers are free functions. A method call never binds at a counted tier (design D2),
so M1 does not measure that cost to recall. Rust re-implementation recall is 4 of 4, above the 50%
that would start phase 1e (a call-position arm for `unique` edges).

## Cost

66a3107 is the commit of this work before its rebase onto main 34adcc9. Its rebased copy is
e021cb8, with the same diff.

The release build of 66a3107, 3 runs of each command, an empty cache directory for each run, and
the 1-minute load at the start of each run. Source: `/var/tmp/claude-rust-port/co1/logs/timing-c3.tsv`
(work record of ticket CO1a).

| Repository | `--since` | `callee_overlap` | peak memory | `clone_delta` | peak memory | load |
|---|---|---|---|---|---|---|
| this repository at 66a3107 | HEAD~5 | 0.67-0.70 s | 382-399 MB | 0.36 s | 104 MB | 0.66-1.33 |
| this repository at 66a3107 | HEAD~50 | 0.80 s | 449-464 MB | 0.25-0.26 s | 172 MB | 1.30-1.33 |
| aiohttp 50cccb38 (144 .py), one added function | HEAD | 0.08 s | 64-67 MB | 0.09-0.10 s | 20 MB | 1.30 |
| checkov 24535627 (4,495 .py), one added function | HEAD | 0.81-0.82 s | 292-296 MB | 0.60 s | 39-40 MB | 1.28-1.30 |

The check parses every file of the tree on each run, with its references, and holds the whole
symbol graph. That is why it takes 2-7 times the memory of `clone_delta`, which reads only the
clone blocks of the source files.

## A false edge found while timing

`--since HEAD~50` on this repository gave 36 rows with the build of 66a3107. In 4 of them the shared
set held a test helper: a Rust call binds every same-file definition of its name, and a helper in a
`#[cfg(test)]` module shares the module scope of the file (`xtask/src/repo.rs` `git#2`,
`xtask/src/ground_truth/lints.rs` `table#2`). Code outside the tests cannot call test code, so the
check now counts no test function as a callee (commit "Count no test function as a callee of
callee_overlap"). The same run then gives 32 rows. On the rebased branch (on main 34adcc9) it gives
31: #66 made `patches` in crates/cli/src/lint.rs call `failed`, so `patches` shares 3 of the 5
callees of `pairs` (60%), and 2 other rows change their counts. Nobody read these 31 rows as one
set: M3 reads the range of each merged pull request instead.

## The measurement of M2 to M4

All runs: the release build of main 81d5a7e, an empty cache directory under the work directory, a
detached worktree or a copy for each tree, and exit 0 for every run. Source:
`/var/tmp/claude-rust-port/co1b/status.md` and its `key/` and `out/` directories (work record of
ticket CO1B). Every prediction was written before its run.

### M2: history

Of the 402 non-merge commits of main, 97 delete 1 to 5 functions and add no function of the same
name. 4 pairs meet the rule of the plan: a commit deletes d, and every call site of d then calls an
existing g. 2 come from the 97, and 2 from commits that delete more than 5 functions. The check
reports none of the 4. Recall is not measured: the plan needs 10 pairs. The causes:

| d (deleted) | g (kept) | why no row |
|---|---|---|
| `build_scopes` (Python) | `build_file_scopes` | d is a 1-line wrapper with 1 counted callee; the minimum is 3 |
| `_ref_role` (Python) | `_python_role` | d calls g |
| `setup` (Rust) | `git` | d is a test function |
| `temporary_file` (Rust) | `create_named` | one job in two homes, but every shared call is a method call on a receiver, which design D2 never counts |

In a sample of 23 deletions, the code that replaces the deleted function calls a function that the
same commit adds. Such a pair does not meet the rule, so this history holds few pairs.

### M3: merged pull requests

36 merged pull requests: #1 to #61 and #63 to #67, without #62 and #68. Each runs over its own
range: a worktree at the merged branch tip, and `--since` the merge base.

- 19 rows from 10 pull requests: #20 2, #24 1, #26 6, #30 1, #50 2, #51 2, #58 1, #59 1, #64 1 and
  #66 2. 13 rows are rewritten functions and 6 are new functions.
- #62 adds the whole Rust tree. It runs apart and gives 60 rows (31 Rust, 29 Python). Nobody read
  them.
- #68 adds the planted pairs of M1, so it is not in the set.
- `clone_delta` over the same ranges reports none of the 19 functions, and none of the 60 of #62.

### M4: whole-repository sample

The 4 snapshots of the specification: this repository at c2daf41, ripgrep-2576, tokio-6752 and
matplotlib-13989. Each copy holds the tracked files over an empty root commit, and
`lint --since <root> --only callee_overlap` then counts every function as new and searches the whole
tree for g. The rows: 32, 11, 10 and 12, a total of 65. `clone_delta` also reports 4 of the 65
functions. A seeded sample (seed 20261104, at most 15 for each snapshot) gave 60 rows, and 41 of
them filled the reading set: matplotlib 11, this repository 10, tokio 10, ripgrep 10.

### Labels and the stop rule

The reading set holds 60 findings: the 19 of M3 and 41 of M4, in a seeded order. Each finding is a
blind packet: the two bodies and the shared callees with their definitions, but not the tier, the
score, the source, or which function is new. A script checked every packet for those terms. Two
agent readers labeled the findings in two rounds, each without the other's labels.

| Reader | round | duplicate | sibling | false_edge |
|---|---|---|---|---|
| 1 | first 20 | 5 | 10 | 5 |
| 2 | first 20 | 6 | 9 | 5 |
| 1 | all 60 | 12 | 31 | 17 |
| 2 | all 60 | 13 | 30 | 17 |

After the first 20, the readers agree on 19 (Cohen's kappa 0.92). On the other one, one
sibling-or-duplicate call, both said that their confidence was low. Count that finding as a
duplicate, and 14 of 20 are false positives: 70%, Wilson 95% interval 48.1% to 85.5%. The lower
bound is above 40%, so stop rule S1 ended the measurement of the bars there. The owner accepted the
readers' labels, so the owner's sample of 20 findings did not run (2026-11-04).

The owner then asked for the other 40, to test a ranking (next section). Over all 60, the readers
agree on 59 (kappa 0.97, the same disputed finding), and 47 of 60 are false positives: 78.3%,
Wilson 95% 66.4% to 86.9%. By source: the 19 M3 findings are 2 duplicates, 16 siblings and 1 false
edge. The 41 M4 findings are 10 duplicates, 14 siblings, 16 false edges and the disputed one.

### Causes

- Siblings, 30 or 31 of 60: the two functions use the same helpers for a different job, for example
  two steps of one pipeline or two members of one family of functions. Shared callees alone cannot
  tell these from a duplicate.
- False edges, 17 of 60, 16 of them in Rust: a listed callee is not a call to that definition. Both
  readers named the same callees in each of the 17.
  - Rust, a field (7 findings): ripgrep `heap_limit`, `absolute_byte_offset`, `binary_byte_offset`,
    `after_context`, `before_context` and `passthru`, and `required` in this repository, are struct
    fields. Each binds to a function or method of the same name. In two of the 7, a second callee
    is also false: a trait method, and another function of the same name.
  - Rust, a bare call (6): `drop(guard)` in tokio `sync/broadcast.rs` calls `std::mem::drop`, and it
    binds to the 5 `Drop::drop` methods of the file.
  - Rust, a parameter (3): six functions in `crates/app/src/verdicts.rs` take a parameter
    `record: &Pairs` and pass it to `get`, and each use binds to the function `record` in the same
    file (2 findings). `segments` (#64) is the third.
  - Python (1): matplotlib `detrend_none` is a default argument value. Neither body calls it.
- The Rust false edges come from the reference index, not from the check. `refs
  crates/app/src/verdicts.rs::record` at c2daf41 lists 26 same-file references, and at least 13 of
  them (lines 179-247) are the parameter. The group says "locals not checked": the index has scope
  analysis for Python only (`capabilities`).
- Without the 17 false edges, 12 or 13 of 43 findings are duplicates (28% to 30%, derived). A fix of
  the reference index alone does not let bar A pass.

### A confidence ranking

The question: does a feature of the row pick out the duplicates, so that a report can show only the
most likely ones as plausible duplicates? The first 20 labels suggested one: every shared callee is
in the same file as both functions. The other 40 findings, read after that, test it.

| Set | rows with the feature | duplicates among them | rows without | duplicates among them |
|---|---|---|---|---|
| first 20 (where it was found) | 6 | 3, and the disputed one | 14 | 2 |
| other 40 (held out) | 7 | 3 | 33 | 4 |
| all 60 | 13 | 6, and the disputed one | 47 | 6 |

On the held-out 40 the feature points the right way (43% against 12%), but 7 rows are few: a
one-sided Fisher test gives p = 0.088. Over all 60, 6 or 7 of the 13 rows are duplicates, and none
is a false edge. No other feature tested (5 or more shared callees, a rewritten function, the
language, both functions in one file) separates the labels on the held-out 40.
