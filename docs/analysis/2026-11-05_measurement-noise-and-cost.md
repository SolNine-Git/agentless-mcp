# Measurement noise, test size and the cost measure

Date: 2026-11-05. Measured on stored results only: no new session ran for this record. The three scripts
are in the appendix, with their sha256. The data comes from SWE-Explore-Bench runs (the sibling clone
`../swe-explore-bench`) with Claude Code and the model `sonnet`.

## Why this record exists

The owner asked three questions: does the method measure the real value of the tools, what is the smallest
test that gives a valid result, and how do we capture and store the data. This record answers the parts that
the stored data can answer. It also lists the flaws in the method that the answers exposed. The new
measurement standard and the code-correctness experiment get their own documents.

## The data

| Run | Date | Build | Tasks | Arms, in run order |
|---|---|---|---|---|
| conf2 | 2026-10-29 | ef6a1a2 | bench300, 300 | native, ungated, forced, hooked |
| PH2 screen | 2026-11-04 | 81d5a7e, Claude Code 2.1.318 | flat60, 60 | native, enrich, native_placebo, hooked, compact, one after another |
| PH2C, native arm | 2026-11-04 | 81d5a7e, Claude Code 2.1.318 | bench300, 300 | native (the enrich arm ran after this record) |

The arms:
- **native:** Read, Glob and Grep only.
- **ungated:** the agentless MCP tools beside the native tools, with no gate.
- **forced:** the agentless tools, with Grep and Glob removed.
- **hooked:** the agentless tools with the search gate.
- **enrich:** native, plus the UserPromptSubmit hook that puts the map's top 2 files into the prompt.
- **native_placebo:** native again, the same command and environment.

flat60 is a subset of bench300.

## Method

- Paired difference: treatment minus control on the same task.
- Smallest detectable effect: MDE(n) = 2.8 x SD / sqrt(n), where SD is the standard deviation of the paired
  differences (80% power, two-sided 0.05, normal approximation).
- Context per session: input + cache write + cache read tokens, summed over the session's requests. The
  screens and confirmations reported cost with this measure.
- Priced cost per session: input + 1.25 x cache write + 0.1 x cache read + 5 x output tokens, in
  input-equivalent tokens (the standard price ratios with the 5-minute cache write). Also Claude Code's own
  `total_cost_usd`.
- Intervals: bootstrap 95%, 10,000 resamples, seed 20261104.

## Results

### 1. Noise and the smallest detectable effect (noise.py)

WCC is weighted core coverage, the bench's main coverage metric. The native mean is 0.118 on flat60 and 0.122
on bench300.

| Pair | n | WCC difference [95%] | SD | MDE, 100 tasks | MDE, 300 tasks | Priced cost [95%] | Priced MDE, 100 tasks |
|---|---|---|---|---|---|---|---|
| placebo, one window (flat60) | 60 | +0.0160 [-0.0019, +0.0405] | 0.0857 | 0.0240 | 0.0139 | -16.7% [-23.8%, -9.8%] | 7.9% |
| placebo, two windows (bench300) | 300 | -0.0012 [-0.0101, +0.0073] | 0.0765 | 0.0214 | 0.0124 | -5.3% [-8.6%, -2.3%] | 7.9% |
| enrich - native (flat60) | 60 | +0.0367 [+0.0129, +0.0664] | 0.1078 | 0.0302 | 0.0174 | -6.2% [-13.8%, +2.9%] | 9.2% |
| hooked - native (flat60) | 60 | +0.0141 [-0.0044, +0.0356] | 0.0812 | 0.0227 | 0.0131 | +48.0% [+33.7%, +62.9%] | 16.1% |
| hooked - native (conf2) | 300 | +0.0054 [-0.0049, +0.0169] | 0.0955 | 0.0267 | 0.0154 | +53.3% [+48.4%, +58.2%] | 12.1% |
| forced - native (conf2) | 300 | +0.0019 [-0.0098, +0.0143] | 0.1054 | 0.0295 | 0.0170 | +22.8% [+16.3%, +30.1%] | 17.4% |

Findings:
- An arm of 100 tasks detects a WCC change of 0.021 to 0.030. An arm of 300 tasks detects 0.012 to 0.017.
- Two native sessions of the same task correlate at 0.86 to 0.87.
- The placebo SDs (0.077 and 0.086) are close to the treatment SDs (0.081 to 0.108). Most of the noise is
  sampling from one session to the next, not a difference between treatments. Repeated sessions and added
  tasks buy about the same power per session. Added tasks generalize further.
- At the conf2 SD, the gate's WCC effect of +0.0054 needs about 2,450 tasks (derived: (2.8 x 0.0955 /
  0.0054)^2). The benchmark has 848.
- hit_file_rate is identical in 70% to 82% of the pairs. It is a coarse metric.

### 2. Tasks that never vary (inert.py)

20 of the 300 bench300 tasks (7%) have the same WCC in 5 independent sessions (the 4 conf2 arms and the PH2C
native arm). 2 of the 20 have WCC 0 in all 5. Removing them saves 7% of a run.

### 3. The cost measure (priced_conf2.py)

| conf2 arm against native (n=300) | Context | Priced [95%] | Claude Code USD [95%] |
|---|---|---|---|
| ungated | +34.7% | +3.9% [+0.2%, +7.6%] | +0.9% [-2.8%, +4.8%] |
| forced | +46.0% | +22.8% [+16.3%, +30.1%] | +26.5% [+19.9%, +34.0%] |
| hooked | +115.8% | +53.3% [+48.4%, +58.2%] | +50.6% [+45.8%, +55.5%] |

The context column comes from the conf2 token tables of 2026-10-29, not from priced_conf2.py. The native
session's priced cost is 14,408 input-equivalent tokens: cache write 6,925, cache read 2,192, output 5,285,
input 7. The change per class (derived from the class means):
- forced: cache write +3,116, cache read +1,013, output -840.
- hooked: cache write +4,380, cache read +2,828, output +465.
- ungated: cache write +109, cache read +944, output -500.

Findings:
- The context measure overstates the money cost about 2x for the hooked and forced arms, and about 9x for the
  ungated arm. Most context tokens are cache reads, which cost a tenth of the input price.
- The forced arm writes 15.9% fewer output tokens. That saves about 840 input-equivalent tokens per session,
  5.8% of the native session's priced cost. Its added cache writes and reads cost about 4,100.

### 4. The order bias of the priced cost

The PH2 screen ran its arms one after another in one window. The native_placebo arm ran third, with the same
prompt prefix as native, and its priced cost was 16.7% [-23.8%, -9.8%] lower than native's. The enrich arm
ran second, with the same prefix, and its priced cost was 6.2% lower while its context was 8.9% higher. The
later arms read the prompt cache that the native arm wrote. A cache hit moves tokens from cache write to cache
read, and the context measure counts both at one weight, so context has no such bias. The two-window placebo,
conf2 native against PH2C native 6 days apart, moved the priced cost by -5.3% [-8.6%, -2.3%] and WCC by
-0.0012 [-0.0101, +0.0073].

## Consequences for the method

1. A screen runs 100 tasks per arm, from a fixed set that is chosen without past outcomes. A decision needs a
   confirmation on tasks that the screen did not use.
2. A WCC effect below about 0.02 is not measurable at 300 tasks, and one below about 0.01 is not measurable on
   the whole benchmark. Report such an effect as "no effect found", not as a small gain.
3. Report context and priced cost side by side. A priced cost decides a question only when the arms alternate
   task by task. A context result is valid in either order.
4. Every run has a placebo arm.
5. The bench scores only the answer list, never code. The code-correctness experiment adds an outcome that the
   tests decide.

## Limits

- The SDs come from three runs and six pairs. A new treatment can have a larger SD.
- The MDE formula is a normal approximation. At 30 tasks it understates the needed sample slightly.
- The priced weights assume the 5-minute cache write (1.25x). A 1-hour cache write costs 2x.
- conf2's Claude Code version is not recorded in this record's sources.

## Repeat

The scripts use the Python standard library only (3.10 or later). On the measuring machine, `<ph2s>` and
`<ph2c>` were `/var/tmp/claude-rust-port/ph2s` and `/var/tmp/claude-rust-port/ph2c`.

```bash
python3 -I noise.py <bench>/results <ph2s>/results <ph2c>/results
python3 -I inert.py <bench>/results <ph2c>/results
python3 -I priced_conf2.py <bench>/results
```

## Appendix: the scripts

### noise.py

sha256 `7a4413f3b395170b9ea74a560201552eb0f4ec85f2dbae629479855cf41c3d9f`

```python
"""Per-task noise of paired differences, and the smallest effect each n detects.

MDE(n) = (1.96 + 0.84) * SD(paired difference) / sqrt(n): 80% power, two-sided 0.05.
Priced cost: input + 1.25 x cache write + 0.1 x cache read + 5 x output tokens.
Bootstrap 95% intervals: seed 20261104, 10,000 resamples. Reads the results read-only.
Usage: noise.py <bench results> <PH2 screen results> <PH2C results>.
"""

import json
import math
import random
import statistics
import sys
from pathlib import Path

# (label, root index, control arm, root index, treatment arm); roots are the 3 arguments.
PAIRS = [
    (
        "placebo, one window (flat60)",
        1,
        "claude_code_ph2_native",
        1,
        "claude_code_ph2_native_placebo",
    ),
    ("placebo, two windows (bench300)", 0, "claude_code_conf2", 2, "claude_code_ph2c_native"),
    ("enrich - native (flat60)", 1, "claude_code_ph2_native", 1, "claude_code_ph2_enrich"),
    (
        "hooked - native (flat60)",
        1,
        "claude_code_ph2_native",
        1,
        "claude_code_agentless_hooked_ph2_hooked",
    ),
    ("hooked - native (conf2)", 0, "claude_code_conf2", 0, "claude_code_agentless_hooked_conf2"),
    ("forced - native (conf2)", 0, "claude_code_conf2", 0, "claude_code_agentless_forced_conf2"),
]
METRICS = ("weighted_core_coverage", "hit_file_rate")
NS = (30, 60, 100, 150, 200, 300, 600)
Z = 1.96 + 0.84
LOW, HIGH, RESAMPLES = 0.025, 0.975, 10_000


def ci(d: list[float]) -> tuple[float, float]:
    rng = random.Random(20261104)
    m = len(d)
    means = sorted(sum(d[rng.randrange(m)] for _ in range(m)) / m for _ in range(RESAMPLES))
    return means[int(LOW * RESAMPLES)], means[int(HIGH * RESAMPLES)]


def top(d: str) -> dict[str, dict]:
    with Path(d, "top10.jsonl").open() as f:
        return {r["instance_id"]: r["metrics"] for r in map(json.loads, f)}


def priced(d: str) -> dict[str, float]:
    out = {}
    with Path(d, "runlog.jsonl").open() as f:
        for r in map(json.loads, f):
            if not r.get("error"):
                out[r["instance_id"]] = (
                    r["input_tokens"]
                    + 1.25 * r["cache_creation_input_tokens"]
                    + 0.1 * r["cache_read_input_tokens"]
                    + 5 * r["output_tokens"]
                )
    return out


def correlation(a: list[float], b: list[float]) -> float:
    if statistics.pstdev(a) and statistics.pstdev(b):
        return statistics.correlation(a, b)
    return float("nan")


def row(name: str, a: list[float], b: list[float]) -> str:
    d = [y - x for x, y in zip(a, b, strict=True)]
    sd = statistics.stdev(d)
    same = sum(1 for x, y in zip(a, b, strict=True) if x == y) / len(a)
    mde = " ".join(f"{n}:{Z * sd / math.sqrt(n):.4f}" for n in NS)
    lo, hi = ci(d)
    return (
        f"  {name}: n={len(d)} mean {statistics.mean(d):+.4f} [{lo:+.4f}, {hi:+.4f}] "
        f"sd {sd:.4f} (control mean {statistics.mean(a):.4f}); identical {same:.0%}; "
        f"corr {correlation(a, b):.2f}; MDE {mde}"
    )


def cost_row(pa: dict[str, float], pb: dict[str, float]) -> str:
    ids = sorted(pa.keys() & pb.keys())
    d = [pb[i] - pa[i] for i in ids]
    sd, base = statistics.stdev(d), statistics.mean(pa[i] for i in ids)
    lo, hi = ci(d)
    mde = " ".join(f"{n}:{100 * Z * sd / math.sqrt(n) / base:.1f}%" for n in NS)
    return (
        f"  priced cost: n={len(d)} mean {100 * statistics.mean(d) / base:+.1f}% "
        f"[{100 * lo / base:+.1f}%, {100 * hi / base:+.1f}%] sd {sd:,.0f} "
        f"(control mean {base:,.0f}); MDE as % of control {mde}"
    )


def main() -> None:
    roots = sys.argv[1:4]
    for label, ia, arm_a, ib, arm_b in PAIRS:
        ca, cb = f"{roots[ia]}/{arm_a}", f"{roots[ib]}/{arm_b}"
        ta, tb = top(ca), top(cb)
        ids = sorted(ta.keys() & tb.keys())
        print(label)
        for m in METRICS:
            print(row(m, [ta[i][m] for i in ids], [tb[i][m] for i in ids]))
        print(cost_row(priced(ca), priced(cb)))


if __name__ == "__main__":
    main()
```

### inert.py

sha256 `2fc7d010bfe80a202e00615474400589ac209a249065bbd1b464fbaf43c1e514`

```python
"""Count bench300 tasks whose WCC never varies across 5 independent sessions.

The sessions: the 4 arms of conf2 and the PH2C native arm. Reads the results read-only.
Usage: inert.py <bench results> <PH2C results>.
"""

import json
import sys
from pathlib import Path

CONF2 = [
    "claude_code_conf2",
    "claude_code_agentless_conf2",
    "claude_code_agentless_forced_conf2",
    "claude_code_agentless_hooked_conf2",
]
M = "weighted_core_coverage"


def top(d: str) -> dict[str, float]:
    with Path(d, "top10.jsonl").open() as f:
        return {r["instance_id"]: r["metrics"][M] for r in map(json.loads, f)}


def main() -> None:
    bench, ph2c = sys.argv[1:3]
    dirs = [f"{bench}/{arm}" for arm in CONF2] + [f"{ph2c}/claude_code_ph2c_native"]
    runs = [top(d) for d in dirs]
    ids = sorted(set.intersection(*(set(r) for r in runs)))
    values = {i: [r[i] for r in runs] for i in ids}
    inert = [i for i, v in values.items() if len(set(v)) == 1]
    zero = [i for i in inert if values[i][0] == 0]
    share = len(inert) / len(ids)
    print(
        f"tasks {len(ids)}; WCC identical in all {len(dirs)} sessions: {len(inert)} ({share:.0%}); "
        f"of them WCC 0 in all: {len(zero)}"
    )


if __name__ == "__main__":
    main()
```

### priced_conf2.py

sha256 `908fe7527801995b5e9c851bfdb6294333c3e1b420919dff8071b0b14ce738bd`

```python
"""conf2 cost per session priced by token class, paired against native.

Input-equivalent tokens: input + 1.25 x cache write + 0.1 x cache read + 5 x output (the standard
price ratios). Also Claude Code's own total_cost_usd. Bootstrap 95%: seed 20261104, 10,000.
Reads the bench results read-only. Argument: the bench results directory.
"""

import json
import random
import statistics
import sys
from pathlib import Path

ARMS = {
    "native": "claude_code_conf2",
    "ungated": "claude_code_agentless_conf2",
    "forced": "claude_code_agentless_forced_conf2",
    "hooked": "claude_code_agentless_hooked_conf2",
}
LOW, HIGH, RESAMPLES = 0.025, 0.975, 10_000


def load(base: str, label: str) -> dict[str, dict]:
    rows = {}
    with Path(base, label, "runlog.jsonl").open() as f:
        for line in f:
            r = json.loads(line)
            if r.get("error"):
                message = f"{label}: error row {r['instance_id']}"
                raise ValueError(message)
            rows[r["instance_id"]] = r
    return rows


def priced(r: dict) -> float:
    return (
        r["input_tokens"]
        + 1.25 * r["cache_creation_input_tokens"]
        + 0.1 * r["cache_read_input_tokens"]
        + 5 * r["output_tokens"]
    )


def usd(r: dict) -> float:
    return r["total_cost_usd"]


def ci(d: list[float]) -> tuple[float, float, float]:
    rng = random.Random(20261104)
    m = len(d)
    means = sorted(sum(d[rng.randrange(m)] for _ in range(m)) / m for _ in range(RESAMPLES))
    return sum(d) / m, means[int(LOW * RESAMPLES)], means[int(HIGH * RESAMPLES)]


def main() -> None:
    data = {a: load(sys.argv[1], label) for a, label in ARMS.items()}
    common = sorted(set.intersection(*(set(d) for d in data.values())))
    print("n common", len(common))
    for arm in ("ungated", "forced", "hooked"):
        for name, f in (("priced", priced), ("usd", usd)):
            base = statistics.mean(f(data["native"][i]) for i in common)
            m, lo, hi = ci([f(data[arm][i]) - f(data["native"][i]) for i in common])
            print(
                f"{arm} - native {name}: {m:+.4f} [{lo:+.4f}, {hi:+.4f}] = {100 * m / base:+.1f}% "
                f"[{100 * lo / base:+.1f}%, {100 * hi / base:+.1f}%]"
            )


if __name__ == "__main__":
    main()
```
