# Phase 8 — Consensus, Remediation Class, and Refactor Gate

Inputs: 24 block reviews (Phase 4), three panel validators (Phase 5), synthesis
patterns (Phase 6). Adversarial verdicts (Phase 7) are merged in the final report.

## 8a. A note on the vote formula before the scores

The skill's formula scores a finding by how many of the three personas flagged it.
Applied literally here it produces a systematic distortion, and the distortion runs
in the dangerous direction: **the most severe findings in this audit are the ones
only one persona has a lens for.**

`git worktree add` running the analysed repository's own git hooks is a security
finding; the Architect has no reason to raise it and the Reliability Engineer has no
reason to raise it. It scores 1/3 = LOW_CONFIDENCE. It was also *executed* — a
`post-checkout` hook wrote a file outside the worktree at exit 0.

So every finding below carries two labels:

- **Panel score** — the formula, reported as specified.
- **Evidence** — REPRODUCED (a block reviewer ran it and observed the failure),
  MEASURED (a number was taken from a real run), or INFERRED (read from the code).

Where the two disagree, evidence governs. A reproduced defect is not less real
because only one specialist was looking for it.

## 8b. Consensus table

Panel abbreviations: A=Architect, R=Reliability, S=Security. Weights: HIGH conf=1.0,
MEDIUM=0.7, LOW=0.4. +0.2 where the finding also appears in a SYSTEMIC or COUPLING
synthesis pattern.

| # | Finding | Panels | Score | Panel label | Evidence | Severity |
|---|---------|--------|-------|-------------|----------|----------|
| 1 | MCP client-advertised roots unioned into the authorization allowlist (`server.py:187`) — `--root` confines nothing | S(C), A(H) | 2.2 | MEDIUM | REPRODUCED twice independently (B16-H1, B22-H1) | CRITICAL |
| 2 | Uncapped repo-controlled config warnings escape the token budget (`envelope.py:127,136`, `projectconfig.py:201-205`) | R(C), S(H) | 2.2 | MEDIUM | MEASURED: 301,789 tokens from a `max_tokens=16000` call, body 100% dropped | CRITICAL |
| 3 | MCP adapter validates none of what the CLI validates, across 6+ services | A(C), S(M) | 2.2 | MEDIUM | REPRODUCED in 6 blocks (B11,B12,B17,B18,B19,B22) | CRITICAL |
| 4 | `git worktree add` runs the analysed repo's git hooks on the default `patch apply` path (`sandbox.py:184-202`) | S(C) | 1.0 | LOW | REPRODUCED: hook executed, wrote outside the worktree, exit 0 (B14-M2) | CRITICAL |
| 5 | `validate`'s `test_cmd` falls back to the analysed repo's own `.agentless-mcp.json` | S(C) | 1.0 | LOW | CONFIRMED in 3 blocks (B14-M3, B21-M4, B23-H5); contradicts the module's own docstring | CRITICAL |
| 6 | Stale `CachedSource` snapshot returns `[]` symbols/refs with a `fresh` receipt (`cache.py:320-368`) | R(C) | 1.0 | LOW | REPRODUCED against a real database (B08-C1) | CRITICAL |
| 7 | In-place apply writes back lossily-decoded text, corrupting non-UTF-8 bytes with `ok=True` (`patch_service.py:339-343`) | R(H) | 1.2 | LOW | MEASURED: `b'# caf\xe9\n'` → `b'# caf\xef\xbf\xbd\n'`, in place, exit 0 (B20-C1) | CRITICAL |
| 8 | `walk_repo`'s git path follows tracked symlinks outside the repo root (`treewalk.py:79-104`) | S(H) | 1.2 | LOW | REPRODUCED: out-of-root file content read and indexed (B04-C1) | HIGH |
| 9 | Four per-child recursive tree walks; one bad file aborts the whole-repo index (`extractor.py`, `cache.py:657`) | R(H), A(H, as SRP) | 2.0 | MEDIUM | MEASURED: RecursionError at 248 chained JS calls | HIGH |
| 10 | Reader `_discard` unlinks the database/WAL mid-write; writer reports success with zero rows | R(H) | 1.0 | LOW | REPRODUCED (B08) | HIGH |
| 11 | `list_roots()` awaited with no timeout on every tool call | R(H), S(M) | 1.9 | MEDIUM | INFERRED (no timeout present) | HIGH |
| 12 | Captured subprocess output unbounded on disk despite a comment claiming a cap | R(H), S(M) | 1.9 | MEDIUM | INFERRED from code + the contradicting comment | HIGH |
| 13 | Test-run infrastructure errors laundered into rankable "applied" verdicts | R(H) | 1.0 | LOW | REPRODUCED: 3 all-error candidates yield tier `applied` + a crowned winner (B21-H1) | HIGH |
| 14 | Patch write loop has no error handling; partial writes leave a half-patched checkout | R(H) | 1.0 | LOW | MEASURED (B20) | HIGH |
| 15 | Killed `git worktree add` leaks a permanent locked record; no scratch GC exists | R(H) | 1.0 | LOW | REPRODUCED: `prune` skips locked records (B14-H1) | HIGH |
| 16 | Silent truncation: only 1 of ~8 bounded listings announces its cut | A(M) | 0.9 | LOW | MEASURED: 42/52 reference sites dropped under a header asserting "10 references" | HIGH |
| 17 | Same knowledge in 2-4 homes, 11 instances already drifted | A(H) | 1.2 | LOW | CONFIRMED across 15 blocks | HIGH |
| 18 | `core` is one opaque import-linter member; `refs -> cache` pulls sqlite3 into 8 modules | A(H) | 1.0 | LOW | CONFIRMED (B09) | HIGH |
| 19 | Relative-import resolution is dead code; every Python `from . import x` loses its 3x edge | — | — | — | CONFIRMED: branch unreachable, uncovered by 1035 tests (B09-H1) | HIGH |
| 20 | `patchlint`'s `DEGRADED_ERRORS` swallows its own internal bug classes as clean "not_checked" lines | R(M) | 0.7 | LOW | INFERRED; the comment states the opposite intent | MEDIUM |
| 21 | Malformed `pyproject.toml` → `known=True` with an empty package set; every dependency reads as hallucinated | — | — | — | REPRODUCED on 3.13 (B15-H1) | HIGH |
| 22 | No CI exists; every contract is enforced only by local hooks | A(L) | 1.0 | LOW | CONFIRMED: no `.github/` (3 blocks) | MEDIUM (meta) |

Rows 19 and 21 carry no panel vote at all: they are block-level findings inside
domains no persona was assigned. Both were confirmed against running code. They are
listed here because a consensus table that omitted them would be measuring the panel
rather than the codebase.

## 8c. Remediation class and the coverage gate

Coverage classes from Phase 3: **REFACTOR-READY** — B02, B11, B18. **COVERAGE-GAP** —
the other 20. **TEST-DESERT** — `application/lint_service.py` (inside B20); B07 has
TEST-DESERT sub-regions (all Rust/C/C++ extraction).

The gate blocks REFACTOR and REDESIGN on untested code. It does not block
FIX-IN-PLACE, and most of the urgent security work here is genuinely FIX-IN-PLACE —
a bounded, obviously-correct change at one call site.

### READY — FIX-IN-PLACE, act now

| Finding | Change | Block/class |
|---------|--------|-------------|
| 4 | Add `-c core.hooksPath=/dev/null -c core.fsmonitor=` to `git worktree add`, matching the hardening already applied to `diff()` twelve lines away | B14 / GAP, but a one-line argv change |
| 1 | Intersect instead of union at `server.py:187`: a client root may select among configured roots, never add one | B22 / GAP |
| 2 | Cap the warning list in `projectconfig.parse` where `MAX_STOPLIST_ENTRIES` already lives, and clamp the assembled header | B16+B04 / GAP |
| 3 | Add `Field(ge=1, le=...)` to every numeric MCP parameter | B22 / GAP |
| 11 | Wrap `await context.list_roots()` in `asyncio.timeout`, fall back to static roots | B22 / GAP |
| 9 | Add `RecursionError` to `cache._plan_index`'s per-file except, reusing `patchlint`'s already-reasoned `DEGRADED_ERRORS` | B07+B08 / GAP |
| 5 | Require an explicit `--allow-config-test-cmd` opt-in for the config fallback | B21 / GAP |
| 8 | Apply the existing containment helper to `_git_listed_paths` output | B04 / GAP |

Each is a guard added at a boundary, not a restructuring. The coverage gate does not
apply, but every one needs a regression test written alongside it — for finding 4
that test is "commit a hook, create a worktree, assert it did not run."

### BLOCKED — characterization tests required first

**REFACTOR items in COVERAGE-GAP or TEST-DESERT blocks. No diffs are offered.**

- **Split `core/extractor.py` (2090 LOC, Ca=14)** — B07 is COVERAGE-GAP with
  TEST-DESERT sub-regions at 73%. Required first: (1) fixtures exercising every
  Rust/C/C++ handler, since none execute today; (2) tests pinning the four node-type
  tables `normalize`/`skeleton`/`patchlint` import from it; (3) a decorated-function
  and PEP-695 alias case. Note the trap: `test_stable_ids.py:117-120` currently
  *asserts* that Go type symbols are absent, so correcting the extractor turns the
  suite red. Audit the goldens against hand-written expectations before touching code.
- **Split `core/patchlint.py` (1761 LOC)** — required first: tests for `_literal_end`
  (the entire string/comment scanner is unpinned), the multi-candidate refusal, and
  the manifest-unreadable paths.
- **Restructure `application/lint_service.py`** — TEST-DESERT. Its 93% is entirely
  incidental (2 rendering goldens + 5 CLI exit-code smoke tests, zero tests of any
  behavior it owns). Required first: tests for `load_candidates` stem collisions and
  non-UTF-8 handling, and for each finding-classification path.
- **Move the `FileSource` protocol out of `cache.py` to break `refs -> cache`** —
  touches 8 downstream modules. Required first: a test constructing each consumer
  against a stub source with no sqlite3 import.
- **Unify the duplicated knowledge (finding 17)** — required first: a test pinning the
  current behavior of *each* copy, since 11 have already drifted and merging them
  silently picks a winner.

### REDESIGN — needs a design decision, not just tests

- The root-authorization model (finding 1): the code, two docstrings, and the agent
  guide describe three different models. Rule once, then enforce in code.
- The four questions the codebase currently answers three ways each: git degradation
  policy, patch-parse-failure policy, exit-code contract, and where a numeric bound is
  validated.
- Whether `--root` is confinement or advertisement (findings 1, 5, 8 all resolve
  differently depending on the answer).

### INFORMATIONAL

- No CI (finding 22) is not a code defect, but it is the reason several findings are
  scored Low: a matrix build would have caught the 3.10 path, the Windows collection
  failure, and the cold-cache network dependency in `conftest.py`.
