# callee_overlap: the measurement record

This record holds the measurements of `lint`'s `callee_overlap` check, in the order of the
measurement plan in [the specification](../plans/2026-11-02_callee-overlap-duplicate-check.md)
(section 5). It holds M1 and the cost. M2 (history), M3 (merged pull requests) and M4 (the
whole-repository sample) are not run yet, so the false-positive rate is not measured, and the check
stays advisory. Bar A and bar B (section 5) are open.

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
callees of `pairs` (60%), and 2 other rows change their counts. Nobody has read the 31 rows yet:
M3 and M4 read and label them blind.
