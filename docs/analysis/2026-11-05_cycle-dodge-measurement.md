# cycle_dodge: the measurement record

This record holds the measurements of `lint`'s `cycle_dodge` check, in the order of the measurement plan in
section 8 of the specification (the plan with the owner's answers is
[docs/plans/2026-11-05_cycle-dodge-check.md](../plans/2026-11-05_cycle-dodge-check.md)). It also holds the
paper test (B0) of the second planned check, `mirrored_shapes`, which stopped that check before any code.

Result: over 60 blind findings, 10 are false alarms (16.7%, Wilson 95% 9.3% to 28.0%), with the owner's ruling
on the 4 findings that the readers disputed. The check passes bar A (an advisory check) and fails bar B (a
blocking check). Every false alarm comes from one of three faults in how the check
resolves a Python import. The build is the release build of 6dce8e5, whose tree is the tree of main 0193eb1.

## M1: planted pairs

`lint --since HEAD --only cycle_dodge --json` on `tests/characterization/fixtures/cycle_dodge_py`, run the
test's way (the base committed, the change copied on top and left uncommitted): 5 rows. Their `candidates` value
is equal to the golden. All 5 planted positives are found, each with its planted `via`, and no row is on the 6
negatives.

## M4: the rows on 14 snapshots

Each of the 14 Python snapshots of the pilot (one SWE-Explore-Bench snapshot per project) was copied without
`.git` over an empty root commit, so every local import is new. Command: `lint --since <root> --only cycle_dodge
--json`, with an absolute cache directory per run.

| Snapshot | Rows | Snapshot | Rows |
|---|---|---|---|
| sympy | 1,023 | qutebrowser | 26 |
| astropy | 239 | sphinx-doc | 23 |
| pydata (xarray) | 108 | pytest-dev | 23 |
| django | 80 | internetarchive (openlibrary) | 23 |
| matplotlib | 36 | mwaskom (seaborn) | 15 |
| ansible | 30 | scikit-learn | 8 |
| pylint-dev | 3 | psf (requests) | 1 |

- 1,638 rows in all (the specification predicted 1,000 to 2,500; the pilot counted 1,652). Every row has
  `via` = `new`.
- 5 not-checked rows: 1 cycle_dodge not-parsed row (sympy/sets/sets.py), 2 astropy C files above the 1 MB cap,
  and 2 test files that are not UTF-8.
- Each run took 0.07 to 0.64 s, with at most 539 MB of memory.
- A second sympy run with a new cache gave byte-identical JSON.

## M2: recall on the imports that a comment marks

The pilot found 83 local imports with a comment that names a circular import. The check reports 59 of them:
71.1%, Wilson 95% 60.6% to 79.7% (the specification predicted 65% to 85%).

| Pilot class | Reported / marked |
|---|---|
| dodge (the target reaches back through imports at load) | 56 / 63 |
| local-only (the cycle closes only through other local imports) | 2 / 10 |
| type-only | 1 / 2 |
| no edge | 0 / 8 |

13 of the 83 are in test paths, which the check does not read by design. Over the other 70 the recall is 59/70 =
84.3% (Wilson 74.0% to 91.0%), and 56 of the 57 dodge imports. The one miss in scope is a pilot error: the pilot
resolved `import types` in ansible to a test file.

## The sample and the packets

- 60 rows, seed 20261104, at most 6 per snapshot. The draw gives one row to each snapshot in turn, in a fixed
  snapshot order, until 60 rows are drawn. A snapshot with fewer rows gives its turn to the next one. The
  quotas: psf 1, pylint-dev 3, eight snapshots 5 each, four snapshots 4 each.
- One packet per row: the statement with 3 lines around it and its function signature, and the claimed chain
  back, one hop per line, with the header line of each block that encloses the hop. Each packet names a
  read-only copy of its snapshot without `.git`.
- Hidden: `via`, the rule, the check's name, the pilot class, the comment marks and the snapshot's name in the
  id. A script checked every packet and the reader brief for those terms: 0 hits.
- Each comment that names a circular import or a cycle was replaced with `# [comment removed]` in the trees and
  the packets, after the runs (720 comments). Then every comment on every line that a packet shows was replaced
  too (53 more), because a placeholder in the statement window marked the rows whose author named a cycle.
- One cue is left: 18 statement windows hold a placeholder, and all 7 marked rows are among them (7 of 18
  against 0 of 42). The readers were told that a removed comment is not evidence.

## Labels and the stop rules

Labels: `hidden_cycle` (the statement is an import in a function body, it imports the first hop's file, and each
hop imports the next file when its module loads); `false_edge` (the statement or a hop does not import the file
that the packet names); `not_at_load` (every edge holds, but a hop does not run at load). False alarm (FP) =
`false_edge` + `not_at_load`. Two agent readers (Opus) labeled in two rounds, each without the other's labels.
The scoring script (appendix) was written and tested on made-up labels before any reader started.

| Reader | Round | hidden_cycle | false_edge | not_at_load |
|---|---|---|---|---|
| 1 | first 20 | 13 | 6 | 1 |
| 2 | first 20 | 17 | 2 | 1 |
| 1 | all 60 | 50 | 8 | 2 |
| 2 | all 60 | 54 | 4 | 2 |

After the first 20, the readers agree on 16 (kappa 0.518). The FP rate is 3 of 20 when both readers must say FP
(15.0%, Wilson 5.2% to 36.0%) and 7 of 20 when one reader is enough (35.0%, 18.1% to 56.7%). The lower bound is
below 40%, so stop rule S1 did not stop the measurement.

The 4 disagreements are one case: `from P import N` in a module of P, where N is a submodule that `P/__init__.py`
does not bind, and the check names `P/__init__.py` as the next file. Before round 2, main sent both readers the
same clarification: such a statement loads the submodule file `P/N.py`, so an edge named to `P/__init__.py` is a
`false_edge`. When `P/__init__.py` binds N, the edge holds. The round 1 labels stand as given. Reader 2 reports
that its 4 round 1 labels would be `false_edge` under the clarification. The owner ruled (2026-11-05) that the
4 are false alarms.

Over all 60, the readers agree on 56 (kappa 0.722), and the 4 disagreements are the same 4.

| Count | FP | Wilson 95% |
|---|---|---|
| both readers say FP | 6/60 = 10.0% | 4.7% to 20.1% |
| one reader is enough | 10/60 = 16.7% | 9.3% to 28.0% |
| decided (the owner's ruling on the 4) | 10/60 = 16.7% | 9.3% to 28.0% |

- Bar A (advisory: FP at most 40% over at least 40 findings): PASS, also when every disputed finding is an FP.
- Bar B (block: FP at most 10% with a Wilson upper bound at most 20%, 60 findings, planted recall at least 50%
  per kind, marked recall at least 70%): FAIL. The decided rate, 16.7%, is above 10%. Even the best case,
  which meets 10%, has an upper bound of 20.1%.
- Recorded, not a gate: weighted by each snapshot's share of the 1,638 rows, the best-case FP rate is 1.7%,
  because sympy holds 62% of the rows and none of its sampled rows is an FP.
- The 7 marked rows: 6 are `hidden_cycle` for both readers, and the seventh is one of the 4 disputed.

## Causes

Every false alarm comes from one of three faults in the import resolver. The sample is of rows, not of edges, so
one wrong edge gives several rows: two of the hops below are each in two packets.

| Fault | Packets | Example |
|---|---|---|
| A block under an aliased `TYPE_CHECKING` counts as code that runs at load | 2 (one hop) | `if t.TYPE_CHECKING:` after `import typing as t`, ansible lib/ansible/utils/display.py:60 |
| An absolute import of a standard-library module resolves to a project file of the same name | 3 (two hops) | `from math import sqrt` in sphinx/util/parallel.py taken as sphinx/util/math.py; `from warnings import warn` in src/_pytest/deprecated.py |
| `from P import N` with an unbound submodule N resolves to `P/__init__.py` | 5 (4 disputed) | `from . import constants` in requests' chardet; `from pylint import utils` |

The list `TYPE_CHECKING` in crates/syntax/src/rules.rs holds only `TYPE_CHECKING` and `typing.TYPE_CHECKING`,
which explains the first fault. The other two are in the resolution of an import to a file.

## Cost

| Unit | Tokens (Opus) |
|---|---|
| Packet builder (M1, M2, M4, sample, packets, leak check, the placeholder ruling) | 256,000 |
| Reader 1, both rounds | 213,000 |
| Reader 2, both rounds | 170,000 |
| In all | 639,000 |

The specification estimated 150,000 tokens, and its stop rule S3 stops a measurement at twice its estimate. S3
was not applied. Before U3 started, main estimated it again at about 600,000 tokens from B0's measured cost (below)
and told the owner. The runs are on the owner's Max plan, so the cost is plan usage, not a charge.

## Limits

1. No sample of changes (M3): this repository has no Python change set to sample. Every row here is a whole
   snapshot taken as new, so `via` is always `new`. The rows for a moved import or a new chain back are measured
   only on the planted fixture.
2. The readers are agents. The owner's sample of 20 findings (agent-owner kappa) did not run.
3. The clarification came between the rounds. It changed no label of round 1.
4. One cue is left in the packets (the placeholders above).
5. 14 snapshots of one project each, all Python.

## B0: the paper test of mirrored_shapes

`mirrored_shapes` would report two types in two files with many field names in common. Before any code, two
agent readers labeled 20 blind pairs from the pilot (11 Rust, 9 Python): `mirror` (one record in two homes, and
nothing fails on drift), `tied` (a compiler, generator or test fails on drift), `sibling` or `generic`.
FP = `tied` + `sibling` + `generic`.

- Before reader 2's labels, main ruled that a pair with a converter that fails the compiler on drift in one
  direction counts as `tied`. Main verified each of the 4 such converters in the code.
- The readers agree on 19 of 20 (kappa 0.921 with the ruling, 0.916 without).
- FP with the ruling: 17/20 = 85.0% (Wilson 64.0% to 94.8%). Without the ruling: 13/20 = 65.0% (43.3% to
  81.9%). Both lower bounds are above 40%, so stop rule S1 stopped B0, and `mirrored_shapes` is not built.
- An observation: 4 of the 11 Rust pairs are one record with a struct-literal converter that reads the source by
  field access. A field added to the source compiles, and the copy drops it.
- Cost: 470,000 tokens (packet builder 172,000, readers 136,000 and 162,000); the specification estimated 60,000.

## Reproduce

The work directory held the build, the packets, the key, the labels and the scripts. The labels have these
sha256: reader 1 3e67ecfd..., reader 2 3e6f7c65... (all 60); round 1 1af37f07... and 26236a39.... The scoring
script below has sha256 c5989369.... Its output on the labels is the second table of "Labels and the stop
rules", and the first 20 rows give the round 1 figures.

## Appendix: the scoring script

```python
"""U3 scoring: reader agreement, kappa, the false-alarm rate with its Wilson interval, S1, bars.

Written before any reader label exists. Scores the ids that both readers labeled, which must be the
first 20 or all 60 of order.txt. FP = (false_edge + not_at_load) / read. Best case: both readers say
FP. Worst case: either reader says FP. A bar passes when the worst case passes, fails when the best
case fails, and else waits for the owner's ruling on the disagreements. Optional arguments: the
two label files (default u3-r1 and u3-r2 under ROOT).
"""

import json
import math
import re
import sys
from pathlib import Path

ROOT = Path("/var/tmp/claude-rust-port")
U3P = ROOT / "u3p"
LABELS = ("hidden_cycle", "false_edge", "not_at_load")
FALSE_ALARM = {"false_edge", "not_at_load"}
Z = 1.959964
S1_BOUND = 0.40
ROUND_ONE = 20
BAR_A = {"fp": 0.40, "n": 40}
BAR_B = {"fp": 0.10, "upper": 0.20, "n": 60, "planted": 0.50, "marked": 0.70}
PLANTED = {"found": 5, "planted": 5}
ALIAS_HEADER = re.compile(r"if\s+(?!typing\.)\w+\.TYPE_CHECKING")


def load(path: Path) -> dict[str, dict]:
    rows = {}
    for line in path.read_text().splitlines():
        if line.strip():
            row = json.loads(line)
            if row["label"] not in LABELS:
                message = f"{path}: packet {row['id']} has label {row['label']!r}"
                raise ValueError(message)
            if row["id"] in rows:
                message = f"{path}: packet {row['id']} labeled twice"
                raise ValueError(message)
            rows[row["id"]] = row
    return rows


def wilson(k: int, n: int) -> tuple[float, float]:
    p = k / n
    centre = (p + Z * Z / (2 * n)) / (1 + Z * Z / n)
    half = Z * math.sqrt(p * (1 - p) / n + Z * Z / (4 * n * n)) / (1 + Z * Z / n)
    return centre - half, centre + half


def kappa(a: list[str], b: list[str]) -> float:
    n = len(a)
    observed = sum(x == y for x, y in zip(a, b, strict=True)) / n
    expected = sum((a.count(c) / n) * (b.count(c) / n) for c in LABELS)
    return (observed - expected) / (1 - expected) if expected < 1 else 1.0


def rates(ids: list[str], a: dict[str, str], b: dict[str, str]) -> tuple[int, int]:
    best = sum(a[i] in FALSE_ALARM and b[i] in FALSE_ALARM for i in ids)
    worst = sum(a[i] in FALSE_ALARM or b[i] in FALSE_ALARM for i in ids)
    return best, worst


def line(name: str, k: int, n: int) -> str:
    lo, hi = wilson(k, n)
    return f"{name} {k}/{n} = {k / n:.1%} [{lo:.1%}, {hi:.1%}]"


def bar_a(best: int, worst: int, n: int) -> str:
    if n < BAR_A["n"]:
        return f"not decided (n={n} < {BAR_A['n']})"
    if worst / n <= BAR_A["fp"]:
        return "PASS"
    if best / n > BAR_A["fp"]:
        return "FAIL"
    return "the owner rules on the disagreements"


def bar_b(best: int, worst: int, n: int, marked: tuple[int, int]) -> str:
    if n < BAR_B["n"]:
        return f"not decided (n={n} < {BAR_B['n']})"
    recall_ok = (
        PLANTED["found"] / PLANTED["planted"] >= BAR_B["planted"]
        and marked[0] / marked[1] >= BAR_B["marked"]
    )

    def holds(k: int) -> bool:
        return k / n <= BAR_B["fp"] and wilson(k, n)[1] <= BAR_B["upper"]

    if not recall_ok or not holds(best):
        return "FAIL"
    if holds(worst):
        return "PASS"
    return "the owner rules on the disagreements"


def marked_recall() -> tuple[int, int]:
    rows = [json.loads(x) for x in (U3P / "key/marked.jsonl").read_text().splitlines() if x]
    return sum(bool(r["found"]) for r in rows), len(rows)


def alias_ids(ids: list[str]) -> list[str]:
    return [i for i in ids if ALIAS_HEADER.search((U3P / "packets" / f"{i}.md").read_text())]


def weighted(ids: list[str], a: dict, b: dict, key: dict[str, dict]) -> float:
    pool = {}
    for run in (json.loads(x) for x in (U3P / "key/m4_runs.jsonl").read_text().splitlines() if x):
        pool[run["snapshot"]] = run["rows"]["advisory"]
    total, acc = 0, 0.0
    for snap, size in pool.items():
        mine = [i for i in ids if key[i]["snapshot"] == snap]
        if mine:
            acc += size * rates(mine, a, b)[0] / len(mine)
            total += size
    return acc / total


def main() -> None:
    order = (U3P / "packets/order.txt").read_text().split()
    key = {
        r["id"]: r
        for r in (json.loads(x) for x in (U3P / "key/findings.jsonl").read_text().splitlines() if x)
    }
    paths = sys.argv[1:3] or [ROOT / "u3-r1/labels.jsonl", ROOT / "u3-r2/labels.jsonl"]
    r1, r2 = (load(Path(x)) for x in paths)
    both = [i for i in order if i in r1 and i in r2]
    if both not in (order[:ROUND_ONE], order):
        message = f"scored ids are not the first {ROUND_ONE} or all of order.txt: {len(both)}"
        raise SystemExit(message)
    a = {i: r1[i]["label"] for i in both}
    b = {i: r2[i]["label"] for i in both}
    la, lb = [a[i] for i in both], [b[i] for i in both]
    n = len(both)
    agree = sum(a[i] == b[i] for i in both)
    print(f"U3 scoring, n={n}: agree {agree}/{n}; kappa {kappa(la, lb):.3f}")
    for reader, labels in (("R1", la), ("R2", lb)):
        print(f"  {reader}: " + ", ".join(f"{c} {labels.count(c)}" for c in LABELS))
    best, worst = rates(both, a, b)
    lo_best = wilson(best, n)[0]
    print("  FP " + line("best case", best, n) + "; " + line("worst case", worst, n))
    s1 = "STOP" if lo_best > S1_BOUND else "go on"
    print(f"  S1 (best-case lower bound above {S1_BOUND:.0%}): {s1}")
    marked = marked_recall()
    print(f"  marked recall {marked[0]}/{marked[1]} = {marked[0] / marked[1]:.1%}; planted 5/5")
    print(f"  bar A (advisory): {bar_a(best, worst, n)}")
    print(f"  bar B (block): {bar_b(best, worst, n, marked)}")
    print(f"  recorded, not a gate: pool-weighted best-case FP {weighted(both, a, b, key):.1%}")
    for i in alias_ids(both):
        print(f"  recorded: aliased TYPE_CHECKING hop in {i}: R1 {a[i]}, R2 {b[i]}")
    for i in both:
        if key[i]["marked"]:
            print(f"  recorded: marked row {i}: R1 {a[i]}, R2 {b[i]}")
    for i in both:
        if a[i] != b[i]:
            left, right = r1[i]["confidence"], r2[i]["confidence"]
            print(f"  disagree {i}: R1 {a[i]} ({left}), R2 {b[i]} ({right})")


if __name__ == "__main__":
    main()
```
