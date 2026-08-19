# Codebase Review: agentless-mcp (src/agentless_mcp, 18,245 LOC)

Audit date: 2026-08-19 · started at `b7a97ca`, re-verified at `5ab0eb2` (0.2.0)
Baseline: 1140 passed, 4 skipped, 90% line coverage · re-measured at HEAD: 1141 passed, 90%
Method: 24 isolated Opus block reviewers (7-pass scrutinize each) → 3-persona panel →
Opus cross-block synthesis → adversarial verification on the local coder backend

> **Provenance.** The tree moved twice mid-audit. `024f318` (diagram import edges)
> landed one minute after the coverage baseline was captured and touched only
> `graph_service.py` — block B19, whose reviewer ran later and therefore described the
> post-fix code. `5ab0eb2` was a version bump. Coverage re-measured at HEAD is
> unchanged (90%, 1141 passed), and B19's two findings were re-confirmed present.
> All findings are accurate about HEAD `5ab0eb2`; see `progress.md` for the full
> correction.

## Executive summary

The architecture is sound and the design thinking behind it is unusually strong. The
module graph is a real DAG with no cycles, no lazy imports and no layer inversions;
the boundaries sit in the right places; and nearly every module states a defensible
position before implementing it. That is rare, and it is the finding that shapes
everything else.

What the audit found is not architectural error but an **enforcement gap**. The
codebase's load-bearing guarantees live in docstrings rather than in gates, so each
of its five theses — bounded views that announce their cuts, degradation reported
rather than coalesced, one typed parse per boundary, a trustworthy receipt,
deterministic ordering — holds precisely where a test or lint contract backs it and
fails in two to nine places where it does not. Twenty-one load-bearing false
guarantees were catalogued; three of them actively misdirect a reader rather than
merely being stale.

The highest-leverage work is therefore not a refactor. It is adding CI, two
`forbidden` import contracts and an intra-core layer order, ruling once on the four
questions the codebase currently answers three ways each, and then deleting every
sentence that cannot be gated.

## Health score

Applied literally, the skill's formula (`100 - 25C - 10H - 3M`) returns **0** for a
357-finding audit — it saturates and would return 0 just as readily for a repo with
five criticals as for one with fifty. Reported for completeness; the per-block map
below is the informative version.

**Assessment: NEEDS_ATTENTION.** No finding threatens the correctness of the core
navigation model. Two reach code execution or data exfiltration from untrusted input,
and both are one-line-class fixes.

## Block inventory

| # | Block | LOC | Cov | Class | C | H | M | L | Health |
|---|-------|-----|-----|-------|---|---|---|---|--------|
| B01 | Shared utility leaves | 423 | 84% | GAP | 0 | 1 | 6 | 8 | 72 |
| B02 | Prompt text catalog | 220 | 100% | **READY** | 0 | 0 | 4 | 11 | 88 |
| B03 | AST symbol model | 299 | 100%* | GAP | 0 | 2 | 5 | 4 | 65 |
| B04 | Repo discovery and config | 726 | 85-96% | GAP | 1 | 2 | 8 | 7 | 31 |
| B05 | Output view models | 857 | 96% | GAP | 0 | 1 | 3 | 6 | 81 |
| B06 | Grammar loading | 314 | 92% | GAP | 0 | 2 | 5 | 6 | 65 |
| B07 | Tree-sitter extraction | 2090 | 73% | GAP+DESERT | 1 | 6 | 8 | 6 | 0 |
| B08 | SQLite index cache | 1102 | 92% | GAP | 1 | 3 | 7 | 6 | 24 |
| B09 | Reference index and graph | 548 | 93-95% | GAP | 0 | 3 | 5 | 9 | 55 |
| B10 | Symbol edge resolution | 811 | 97% | GAP | 0 | 3 | 4 | 7 | 58 |
| B11 | Communities and Mermaid | 765 | 98-100% | **READY** | 0 | 1 | 5 | 7 | 75 |
| B12 | Code view extraction | 780 | 95-100% | GAP | 0 | 4 | 6 | 6 | 42 |
| B13 | Patch parse and normalize | 825 | 95-97% | GAP | 0 | 3 | 9 | 6 | 43 |
| B14 | Sandbox and voting | 731 | 81/100% | GAP | 0 | 2 | 6 | 6 | 62 |
| B15 | Patch linting | 1761 | 93% | GAP | 0 | 3 | 7 | 7 | 49 |
| B16 | Context and envelope | 349 | 96-100% | GAP | 1 | 3 | 4 | 5 | 33 |
| B17 | Map and view services | 634 | 88-98% | GAP | 0 | 2 | 5 | 7 | 65 |
| B18 | Symbol and ref service | 646 | 97% | **READY** | 0 | 3 | 5 | 5 | 55 |
| B19 | Graph and diagram service | 565 | 99% | GAP | 0 | 2 | 4 | 6 | 68 |
| B20 | Patch apply and lint | 590 | 84/73% | GAP+**DESERT** | 1 | 4 | 6 | 4 | 17 |
| B21 | Validate and vote | 826 | 91% | GAP | 0 | 3 | 7 | 4 | 49 |
| B22 | MCP adapter | 715 | 84-100% | GAP | 0 | 4 | 6 | 6 | 42 |
| B23 | CLI adapter | 1502 | 76-86% | GAP | 0 | 5 | 6 | 9 | 32 |
| B24 | Composition root | 153 | 71% | GAP | 0 | 3 | 4 | 4 | 58 |

\* `core/imports.py`'s 100% is tautological — the dataclass body executes at import
and no assertion touches `ImportStatement` anywhere in the suite.

Skipped and announced: seven empty `__init__.py` markers, `__main__.py` (6 lines),
`tests/` (mapped for coverage, not itself a review target), `docs/`.

## The coverage number is the most misleading figure in this audit

90% line coverage with 1140 green tests is why these defects survived. The audit found
the specific mechanisms:

- **Eight tests that cannot fail.** `test_graph_service.py:130-136`'s assertion is
  satisfied by an `or group.omitted == 0` disjunct the fixture already meets;
  `test_cli.py:328` asserts constants equal `(0,1,2)`; `test_graph.py` asserts
  `DEFAULT_DAMPING == 0.85`.
- **A curated fixture corpus.** Zero trailing comments, zero nested functions, zero
  non-ASCII, zero tabs, zero CRLF. B12's skeleton bug lives exactly in that gap.
- **Goldens that encode defects.** `repo_go.map.json` contains no Go type symbols and
  `test_stable_ids.py:117-120` *asserts that absence*. Fixing the extractor turns the
  suite red.
- **Tier-1 languages with no fixtures.** `grammars.py:38-40` claims characterization
  coverage for c/cpp/rust; no handler line executes.
- **Coverage that is incidental rather than owned.** `lint_service.py` reads 93% from
  two rendering goldens and five CLI exit-code smoke tests. Zero tests exercise any
  behavior it owns. It is classified TEST-DESERT despite the number.

## Findings: reproduced, not inferred

Every finding below was executed against running code by a block reviewer and then
independently re-verified in Phase 7. The adversarial pass returned **zero false
positives across ten challenged claims** — but it corrected four severities downward
and one measurement.

### CRITICAL

**1. `walk_repo`'s git path follows tracked symlinks outside the repository root**
`core/treewalk.py:79-104` · READY (FIX-IN-PLACE)
A committed symlink `leaked.py -> /etc/hostname` was listed by `walk_repo`, read
through `read_bounded`, and indexed into the persistent tag cache — symbol names,
imports and signatures from an out-of-root file, served through every subsequent tool
call. The only filter applied is `candidate.is_file()`, which follows the link. The
non-git fallback path already enforces containment; only the bulk-listing path is
unguarded. Fix: call the existing `fslimits._file_stays_inside` from `treewalk.py:93`.
Three lines.

**2. Four per-child recursive tree walks abort the whole-repository index**
`core/extractor.py:782-854,957-980,1337-1362,1379-1398` + `core/cache.py:657` · BLOCKED for the refactor, READY for the containment
`RecursionError` reproduced at exactly 248 chained JS method calls — a shape routine
in minified bundles and generated API clients, not an adversarial input. Because
`cache._plan_index` catches only `LanguageUnavailable`, one qualifying file anywhere
in a repository aborts `agentless index` entirely with a raw traceback. `patchlint.py:178-188`
already enumerates `RecursionError` in a reasoned `DEGRADED_ERRORS` tuple; the index
scan never got the same treatment. Fix in two steps: widen the except (one line,
ships now), then convert the walkers to the explicit-stack form `walk_nodes` already
uses and whose stack-safety the module docstring already credits.

### HIGH (downgraded from CRITICAL by the adversarial pass)

**3. `git worktree add` runs the analysed repository's own git hooks**
`core/sandbox.py:184-202`
A `post-checkout` hook executed and wrote outside the worktree at exit 0. This fires
on the default `patch apply` path, before any patch or test command is involved. The
same file explicitly neutralises `color.diff`/`diff.external` twelve lines away
"because a worktree reads the repository's own configuration" — the hazard was known
and the mitigation reached one call and not the other. The adversarial pass added an
amplification the panel missed: `validate_service.py:452` opens a worktree **per
candidate**, so a 50-candidate run executes the hook 51 times. Fix: `-c core.hooksPath=/dev/null`
on the `worktree add` argv.

*Note on the local coder's dissent:* the local backend returned a REFUTED verdict
token on this claim while its own explanation described the vulnerability correctly.
The code supports CONFIRMED, and the executed hook settles it.

**4. MCP client-advertised roots are unioned into the authorization allowlist**
`adapters/mcp/server.py:187`
A server started with `--root /a` served `/b` when a client advertised it; a server
with zero `--root` flags served whatever the client named. `test_mcp_server.py:118`
pins this as intentional, so it is a deliberate design that contradicts its own
documentation in three places (`repo_context.py:11-14`, `server.py:96`,
`docs/agent-guide.md:136-140`). Downgraded from CRITICAL because the MCP transport is
local stdio and the client is generally the user's own agent — but `--root` reads as a
confinement boundary everywhere it is described, and it is not one.

**5. Stale cache snapshot returns zero symbols under a `fresh` receipt**
`core/cache.py:320-368`
Reproduced against a real database: a `CachedSource` whose `entries` snapshot predates
a concurrent index run returns `[]` symbols and `[]` refs for a file that demonstrably
defines symbols — indistinguishable from the legitimate "this file defines nothing"
answer, and carrying a receipt that says `fresh`. The freshness check and the row read
are separate autocommit SELECTs with no BEGIN pinning one WAL snapshot.

**6. In-place apply corrupts non-UTF-8 bytes outside the patched region**
`application/patch_service.py:339-343`
Measured: `b'# caf\xe9\n'` → `b'# caf\xef\xbf\xbd\n'`, written in place, `ok=True`,
exit 0. `read_bounded`'s `errors="replace"` is correct for analysis and wrong for
round-tripping. The fix must be **asymmetric** — `cache.content_digest` deliberately
depends on the lossy form for hash agreement, so a global strict switch breaks the
cache.

**7. Uncapped repository-controlled config warnings escape the token budget**
`application/envelope.py:127,136` + `core/projectconfig.py:201-205`
A `.agentless-mcp.json` with thousands of unknown keys — well under the existing 64 KB
cap — drives the response header past the ceiling and empties the answer body, for
every tool call against that repository, until the file is edited. The block reviewer
measured 301,789 tokens against a `max_tokens=16000` call; the adversarial pass
re-measured 152,065 on its own config. **Both are measured, they disagree by 2x, and
the config shape differs — treat the magnitude as "9.5x to 19x the ceiling" rather
than either single figure.** `projectconfig.py` already caps `stoplist` in the same
function; the warning list was the one repository-controlled channel left uncapped.

**8. Test-run infrastructure errors are laundered into rankable verdicts**
`application/validate_service.py` + `core/vote.py:232-238`
`RunStatus.ERROR` means the test command never started. Three all-error candidates
produce tier `applied`, the detail line "applied cleanly (nothing passed the
regression suite)", and a crowned winner — a confident report where nothing was
measured. The module is careful about exactly this distinction at the baseline level
(`Verdict.not_evaluated` is documented as "inventing evidence" if misused); the care
does not extend to the candidate step.

**9. Relative-import resolution is unreachable dead code**
`core/graph.py:250-265`
`_candidate_bases` gates on `module.startswith(".")` but the extractor strips the
dots, so the `relative_level` branch never executes — uncovered by the full suite.
Every Python `from . import x` and `from ..x import y` silently loses its 3x-weighted
import edge, and the sibling branch cannot normalize `../foo` either, breaking JS/TS
the same way. This degrades the repo map's ranking quality on exactly the intra-package
edges that matter most.

**10. Malformed `pyproject.toml` makes every dependency read as hallucinated**
`core/patchlint.py:510`
`sources.append` is unconditional, so a parse failure leaves `known=True` with an
empty package set and the warning discarded — every third-party import in the patch is
reported as hallucinated. Reproduced on 3.13 with `tomllib`, no fallback involved.
This is the exact outcome the function's own docstring says it prevents.

### Findings the panel missed (adversarial pass, all new)

- **Windows line-ending rewrite.** `patch_service.py:343`'s `write_text` uses the
  default `newline=None`, so every edited file's line endings are rewritten on Windows
  (LF→CRLF, existing CRLF→`\r\r\n`). Distinct from the UTF-8 corruption. Fix:
  `newline=""`.
- **Lock failures escape unclassified.** `util/filelock.py:83-86` converts only
  `BlockingIOError`, so `ENOLCK`/`EOPNOTSUPP` from `fcntl.flock` on NFS, FUSE or
  overlay filesystems escapes `cache._write_lock`'s except and leaves `build_index`
  unclassified.
- **Truncation outside mutual exclusion.** `util/filelock.py:60` opens the lock file
  with mode `"w"` (`O_TRUNC`) *before* acquiring the lock.
- **Hook amplification** (folded into finding 3 above).

### Cleared as a non-finding

B21 flagged the `@cache`d tree-sitter `Parser` shared across `validate --jobs N`
threads as latent under free-threaded Python. The adversarial pass stress-tested it —
8 threads × 30 concurrent parses on tree_sitter 0.26.0, zero mismatches or crashes.
Not a defect. Recorded so it does not get "fixed" on a future pass.

## Systemic patterns

Full detail in `.audit/synthesis.md` (12 SYSTEMIC, 12 COUPLING, 9 DRIFT, 13
ASSUMPTION, 5 ARCH). The five that carry the most blast radius:

**Silent truncation.** `envelope.Truncation` is the right abstraction, built and
wired to exactly one of roughly eight bounded listings. `find_referencing_symbols`
drops 42 of 52 sites under a header asserting "10 references"; the JSON carries
`total`/`limit` and the text carries neither. For a tool whose entire product promise
is "a bounded view you can trust", this is the defect that most directly undercuts the
premise.

**The MCP adapter validates nothing the CLI validates.** Exactly one `Field(` exists
in `server.py` and it carries only a description — no `ge`/`le` on any wire number,
across six services flagged independently by six block reviewers. Inverted ranges
render whole files, `resolution=NaN` ships a bare `NaN` token (invalid JSON), `limit=0`
answers "no references". The services trust "the adapter checks"; for the MCP half of
the front door that is simply false.

**"Could not measure" rendered as "measured, and the answer is negative."** Nine
blocks. Three never-started candidates rank as `applied`; an unparsed manifest makes
every dependency hallucinated; a stale snapshot returns `[]` with a `fresh` receipt.
For a tool an agent trusts to tell it what exists in a repository, this is the most
dangerous category in the audit.

**One piece of knowledge, two to four homes — eleven already drifted.** Line counting
three ways (all wrong identically, a phantom trailing line), `N|` numbering four ways
in three formats, elision counts computed against two different denominators that
contradict each other *within one response*, `load_candidates` twice with different
collision behavior, `_with_source`/`_context` verbatim duplicated across adapters.

**Prose asserts invariants the code contradicts.** Every existing gate checks
structure — key-set sync, verbatim publication, tool-name matching — and none checks
truth. Twenty-one false guarantees, three actively misdirecting: `validate_service`'s
docstring makes the strongest security claim in the package about a path the CLI
violates; `sandbox`'s claims nothing derived from repository content reaches git;
`main.py`'s claims the adapter owns no behaviour while six subcommands bypass the
service layer.

## Architecture

Three structural gaps, all mechanical to close:

**The two-adapters claim is false and unenforced.** `adapters/cli/main.py` has Ce=27
and reaches into 13 core modules; the MCP adapter reaches into 8. The import-linter
contract is a `layers` contract, so `adapters -> core` is legal and nothing catches
it. Cost already paid: three verbatim duplications, two of which have drifted.
Direction: a `forbidden` contract plus shared homes in `application/`.

**`core` is one opaque import-linter member.** No direction is enforced inside the
largest layer, which is how `core/refs.py` (a domain scan module) came to call
`cache.py`'s concrete `effective_source` factory at runtime, pulling `sqlite3` into
eight downstream modules. The `FileSource` Protocol is correctly designed and lives
inside the infrastructure module it exists to decouple from. Direction: declare an
intra-core order; two mechanical moves then unblock both god-module splits.

**The composition root is a hole.** `bootstrap` and `__main__` sit outside the layer
list by design, and the independence contract names only the two adapter packages — so
nothing forbids `bootstrap` importing the MCP adapter. The optional-extra guarantee
rests on a source-substring test (`"import fastmcp" not in source`) that a
`from agentless_mcp.adapters.mcp.server import ...` would sail past. The dynamic
wiring is `Any` to mypy and executed by zero of 1141 tests: a bogus-kwarg construction
and a call to a nonexistent function both type-check clean.

**No CI.** `.github/workflows/` does not exist. Every contract this audit credits the
codebase with — import-linter, mypy strict, ruff, deptry — is enforced only by locally
installed pre-commit hooks. `requires-python = ">=3.10"` is exercised by nothing;
`test_cache.py` cannot even be collected on Windows (`fcntl` at module scope, no
guard) despite `util/filelock.py` carrying a Windows implementation; `conftest.py`
downloads grammars over the network on a cold cache, so the first run anywhere is
non-hermetic.

## Action plan

### Immediate — ships now, one line to three lines each

1. **Contain the extractor RecursionError** — widen `cache._plan_index:661` to
   `patchlint.py:178-188`'s tuple. Turns a whole-repository outage into a per-file
   degradation. *(effort S, blast radius: every index build)*
2. **Cap the envelope header and move config warnings below `ENVELOPE.banner`** — one
   function closes both the ceiling breach and the prompt-injection region where
   repository text currently renders above the untrusted-content banner. *(S, every
   tool response)*
3. **Containment on the git walk branch** — call `fslimits._file_stays_inside` from
   `treewalk.py:93`. Closes the only content-driven exfiltration path. *(S, every
   index and map)*
4. **Ride-alongs, one line each:** `-c core.hooksPath=` in `sandbox.run_git`;
   `newline=""` at `patch_service.py:343`; `except OSError` at `filelock.py:85`.

### Next — bounded, needs a test alongside

5. **Give numeric bounds one owner** at the service methods, plus `Field(ge=1, le=...)`
   on the wire so the JSON-RPC schema itself refuses out-of-range calls. Pair it with
   passing the listing's `total`/`limit` into `render.py:788` instead of the truncated
   tuple — same change, closes the honesty gap and the validation gap together.
6. **Decide the `--root` model.** Intersect, or union behind an explicit
   `--allow-client-roots`. Then rewrite the three false docstrings and delete the
   `server.py:116-117` single-client-root fallback.
7. **Fix the relative-import resolution** in `graph.py:250-265`, and re-baseline the
   resolution-tier tests — they may currently be pinned against the broken resolver.
8. **Add CI.** A 3.10/3.13 × Linux/Windows matrix running the existing hook stack.
   This is what converts the rest of the audit's "enforced by" claims from aspiration
   to fact.

### Blocked on characterization tests — no diffs offered

Per the coverage gate, these are REFACTOR items in COVERAGE-GAP or TEST-DESERT blocks.
The required tests are enumerated in `.audit/consensus.md` §8c. In brief:

- **Split `core/extractor.py`** — needs Rust/C/C++ fixtures first (no handler line
  executes today), tests pinning the four node-type tables three other modules import,
  and a golden audit against hand-written expectations. The Go-type golden actively
  asserts a defect, so the suite goes red before it goes green.
- **Split `core/patchlint.py`** — needs tests for `_literal_end` (the entire
  string/comment scanner is unpinned) and the manifest-unreadable paths.
- **Restructure `application/lint_service.py`** — TEST-DESERT. Needs tests for
  `load_candidates` stem collisions and non-UTF-8 handling before anything moves.
- **Move `FileSource` out of `cache.py`** — touches 8 modules; needs each consumer
  tested against a stub source with no sqlite3 import.
- **Unify the 11 drifted duplications** — needs each copy's current behavior pinned
  first, since merging silently picks a winner.

### Backlog

Rule once on the four questions answered three ways each: git-degradation policy
(notes / raises / a third), patch-parse-failure policy (refuses / downgrades /
proceeds at exit 0), the exit-code contract (identical conditions exit 0 through
`skeleton` and 1 through `slice`), and where a numeric bound is validated. Then delete
every docstring sentence no gate enforces — the audit's most repeated finding is that
this codebase's prose is more ambitious than its enforcement, and the cheapest fix for
half of those is to stop making the claim.

## Adversarial counterpoints

The Phase 7 reviewer upheld all ten challenged claims but disagreed with the panel on
calibration four times, and it was right each time:

- Three CRITICALs became HIGH (client-root union, stale cache, worktree hooks) —
  local-stdio transport and single-user context lower the realistic exposure.
- `validate`'s `test_cmd` fallback dropped to MEDIUM: it requires an explicit
  `validate` invocation, and the CLI does print the chosen command.
- The 301,789-token figure did not reproduce; 152,065 did. Reported as a range.
- The `read_slice` cost claim was judged overstated even though the defect is real.

It also cleared a flagged concurrency risk outright rather than leaving it hanging,
which is the more useful half of an adversarial pass.

## Audit trail

```
.audit/
├── progress.md              block inventory, per-block log, cross-block flags
├── coverage-baseline.json   HEAD b7a97ca, never overwrite
├── findings/B01..B24.md     357 findings, severity-sectioned, with reproductions
├── panel/                   architect.md, reliability.md, security.md (50 findings)
├── synthesis.md             51 cross-block patterns
├── consensus.md             vote scoring, remediation classes, coverage gate
├── adversarial.md           Phase 7 verification table and independent findings
└── report.md                this file
```

Commits are user-initiated; this audit ran no git write commands. Suggested:
`audit: full codebase review — 24 blocks, 357 findings, 2 critical`.
