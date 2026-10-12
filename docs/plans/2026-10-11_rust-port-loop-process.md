# Rust Port: The Build Loop

**Started**: 2026-10-11 | **Applies to**: P1 to P4 | **Owner decisions**: 2026-10-11

This document records how each unit of the port is built, and what each lesson cost. Update the lessons log and the cost log at the end of every unit.

## Roles

| Role | Who | Does |
|---|---|---|
| PM | The main session (Opus) | Writes specs and briefs, rules on findings, keeps the ledger, proposes commits. Writes no production code and reads no source itself. |
| Scoping worker | Sonnet `Explore` subagent | Lists the Python call sites of a unit and the arguments they really pass, with `file:line`. |
| Executor | Opus subagent | Writes one unit (one crate, or part of a large crate), runs the gates, returns a report inline. |
| Optimize reviewer | Opus subagent with the `/optimize` skill | Reviews each completed file, measured mode, and reports findings only. |
| Owner | The user | Approves each commit and each push. Gets a summary of each plan the PM approves. |

## The loop, per unit

1. **Scope.** A scoping worker lists the Python call sites and the arguments they pass. The spec covers what the callers use, nothing more.
2. **Cases.** The committed generator (for example `scripts/stdlib_fixtures.py` for `text` and `json`) writes the inputs and expected outputs from CPython 3.13 into `crates/<crate>/fixtures/*.json`. Tests read the JSON at run time. No test runs Python, and no case is typed by hand.
3. **Brief.** The PM writes the brief: what exists, the owned files, the spec, the definition of done, and the stop rule. Executor rules: `/var/tmp/claude-rust-port/p1/common.md`. A brief for a third-party API includes the crate's real source from `~/.cargo/registry`.
4. **Plan.** The executor writes a short plan. For a design-heavy unit (crate interfaces, the `tags.db` reader, the hook) the PM approves it first and sends the owner a summary. For other units the executor continues without a wait.
5. **Build.** The executor implements the unit until fmt, clippy `-D warnings`, the tests and the gates pass. After 3 failed attempts at the same check, it stops and reports.
6. **Differential check.** For a parity unit, the executor compares Rust with Python over a real corpus (every `.py` file of this repository and mcp-local, or real CLI outputs) and reports each difference class.
7. **Reconcile.** The PM checks the tree against the report: file list, test count, every generated case still present (compare the literals or the JSON), complexity (no function above 10), and clone share (`agentless-mcp health --json --repo crates`, at or below `crates/erosion-baseline.json`; it reads untracked files).
8. **Optimize.** An Opus reviewer runs `/optimize` on each completed file. The PM rules on each finding. A speed change needs a measurement first.
9. **Log.** The PM records the unit's agent tokens, wall time and rulings in the ledger and in the cost log below.

At the end of a phase, three commits, each with the owner's go:
1. The initial build, once every crate of the phase has landed.
2. The `/optimize` batch.
3. The fixes from `/pr-review`.

## Standing rules

- Three commits per phase (see above). Never one per file.
- `/pr-review` runs only at the end of a phase. No review panels, and no gate hardening.
- No speed target before a measured first-pass baseline. Measure, profile, rank the gains by effort, and let the owner pick where to stop.
- Report the agent tokens at each crate boundary. Ask the owner when a unit passes about 1.5M agent tokens without passing its checks.
- One building agent per crate at a time. Up to three crates can build side by side (owner budget rule), each with its own target directory and under the memory cap (`systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0`, `CARGO_BUILD_JOBS=8`, TMPDIR and builds under `/var/tmp`). The local coder and consult models are stopped to free the memory.
- A crate starts when the approved plan of each crate below it fixes the API. It does not wait for those crates to be built.
- Parity stays byte-exact, error texts included.
- Doc comments state what the code does. Never "as Python does": the Python tree leaves the repository in P4.
- Never edit a generated case to make a test pass. Fix the code, or ask.

## Lessons log

| Date | Lesson | Evidence | Rule it produced |
|---|---|---|---|
| 2026-10-11 | Review loops on gate code do not converge. Every round finds a new bypass. | 77 agents, 18.3M tokens, 0 port lines (P0d) | No gate hardening; stop after two non-converging rounds |
| 2026-10-11 | A spec wider than its callers wastes a full cycle. | `round` with negative `ndigits`: no caller passes one; 2 defects found in dead code | Scope each spec from its call sites |
| 2026-10-11 | A model does not know a third-party API. | The coder invented `serde_json` `Formatter` methods | Paste the real source into the brief |
| 2026-10-11 | Models mistranscribe escapes in test data. | Surrogate pairs and control characters in JSON cases | Generate cases by script; never type them |
| 2026-10-11 | A whole-file rewrite to fix errors adds new errors. | 7 clippy errors rose to 17 in one round | Fix rounds use search-and-replace edits |
| 2026-10-11 | A request's sampling settings override the server's tuned ones. | Temperature 0.2 made the consult model loop | Send only the prompt to a local model |
| 2026-10-11 | The complexity gate cannot see untracked files. | `lint --since HEAD` and `health` skipped `json.rs` | Count complexity by hand in the report until the phase commit |
| 2026-10-11 | Subagents cannot write report files. | The optimize agent returned its report inline | Executors return reports inline; the PM saves them |
| 2026-10-11 | The local consult adds nothing on fully tested files. | `text.rs`, `json.rs`: 8 and 15 min, no finding beyond the PM pass | Consult only for design-heavy units |
| 2026-10-11 | Two builders in one crate block each other. | Benchmarks showed 10-22% noise; a PM check could not compile | One builder per crate |
| 2026-10-11 | A spec written from memory of CPython is wrong in the details. | The tokenizer spec erred on the BOM, the error line and the f-string runs; the executor ported the C lexer source instead | Give the executor the upstream source as the authority; generated cases decide |
| 2026-10-11 | The clone ratchet counts duplicated blocks and trait delegates, and nothing ran it per unit. | The uncommitted pycompat work reached 0.030781 against a 0.030380 baseline; the P1 commit would fail pre-push | Check clone share at each reconcile |
| 2026-10-11 | `pre-commit run --all-files` skips untracked files, like the complexity gate. | codespell flagged the `serde_json::ser::Formatter` path in json.rs only when the file was named | At reconcile, run the hooks on new files by name (`--files`) |
| 2026-10-11 | The Edit tool decodes `\uXXXX` text in its parameters into real characters. | It corrupted one generator line; the executor restored it with Write and a byte diff | Write escape-heavy files with Write, then diff against a scratch copy |
| 2026-10-11 | A corpus check gives far more evidence than hand-picked cases. | 175 cases, then 62,456 real files with 0 differences | Every parity unit gets a corpus check |
| 2026-10-11 | Call-site scoping removes much of a plan section, and the plan's line numbers drift. | `io`: the sandbox runner, the lock, `Deadline` and the fakes have no P1 caller; `syntax`: 6 of 7 Python ranges had moved | Write the brief from the scope report, not from the plan |
| 2026-10-11 | A generator run passes `--check` only in the environment that wrote it, unless it fixes every environment value. | `app`: the capabilities fixture held the executor's grammar-cache path; the PM run failed | At reconcile, run each generator's `--check` with other cache directories than the executor's |
| 2026-10-11 | Scratch gate runs write into the user's real caches unless the cache variables point to scratch. | Two tags.db files from `io` and `checks` gate runs appeared in `~/.cache/agentless-mcp` | Set XDG_CACHE_HOME and TREE_SITTER_LANGUAGE_PACK_CACHE_DIR to scratch for every agentless-mcp run |
| 2026-10-11 | An executor removes a license exception that a sibling crate will need. | `syntax` dropped the `foldhash` exception before `store` brought `rusqlite` in | The crate that brings a dependency owns its policy entries |
| 2026-10-11 | A crate directory without a manifest breaks every builder. | The workspace glob `crates/*` failed to load while `crates/prompts/` and later `crates/git/` had files but no Cargo.toml | Create Cargo.toml and src/lib.rs first in a new crate; run workspace gates in a scratch copy |
| 2026-10-12 | Every gate passes on a working tree that a commit cannot reproduce. | `.gitignore` `target/` hid `crates/hook/src/target/tests.rs`; a clean clone of a scratch commit failed to build | Before a phase commit, build and test from a clean clone of a scratch commit |
| 2026-10-12 | `pre-commit run --files` on untracked files reports Passed while a fixer rewrites a file. | ruff-format changed `scripts/cli_fixtures.py` and the hook passed; pre-commit finds a fixer's change with `git diff` | Stage the files in a scratch copy before the pre-commit run |
| 2026-10-12 | Only the full-tree check finds a CI script that the phase breaks. | `gate-selftest.sh` rows 3, 6b and 8n assumed the P0 workspace; no executor owned the script | The PM's pre-commit verification runs every CI job's commands |
| 2026-10-12 | One target directory shared by two copies of the tree gives false results. | Cargo hashes workspace path packages relative to the workspace root, so a second copy reused the first copy's artifacts (`O1`) | One target directory per tree copy |
| 2026-10-12 | Timings in a fresh clone measure git, not the code. | `git status` took 17 to 24 ms per call until `git update-index --really-refresh`, then 1.7 ms; a subprocess timeout quantized every time to 50 ms | Refresh the index in each timing clone; no subprocess timeout in a timing driver |

## Cost log

| Unit | Who wrote it | Agent tokens | Wall time | Notes |
|---|---|---|---|---|
| `pycompat/text.rs` | local coder | 0 | about 50 min | 2 coder rounds; PM rewrote `splitlines` |
| `pycompat/float.rs` | local coder | 0 | about 2 h 10 min | about 1 h lost to pipeline errors, since fixed |
| `pycompat/json.rs`, `order.rs` | local coder, PM refactor | 0 | about 1 h 30 min | Layout refactor: 211 to 160 lines |
| `/optimize` on `json.rs`, `order.rs`, `text.rs` | Opus | 118,573 | 6.3 min | 3 LOW: 1 accepted, 2 deferred to the P1 profile |
| `prompts` scoping | Sonnet Explore | 52,101 | 51 s | 128 templates, plain `{name}` only |
| P1-prompts audit and plan | Opus | 249,896 | 21.3 min | Found the clippy and clone-gate limits; API changed to typed consts |
| Clone-group attribution | Sonnet | 55,703 | 31 s | 62 new clone lines: float 10, json 34, sloc 18 |
| `pycompat` fixtures, generator and fixes | Opus | 380,283 | 27.3 min | All cases kept; clone share 0.0291; JSON differential 0 differences in 44 real outputs |
| `/optimize` on `sloc.rs` | Opus | 175,086 | 10.4 min | HIGH accepted (45% faster, output equal on 57,468 files); MEDIUM deferred |
| `pycompat/sloc.rs` | Opus | 333,839 | 29.2 min | 760 code lines; 0 differences from CPython over 62,456 files |
| `io`, `syntax` scoping | Sonnet Explore | 291,432 | 3.2 min (parallel) | Plan line numbers had drifted; sandbox runner, lock and deadline have no P1 caller |
| `metrics`, `git`, `checks`, `store` scoping | Sonnet Explore | 368,013 | 2.0 min (parallel) | 4 workers at once; each found plan corrections (store: discard on read, 5 s busy wait; metrics: erosion and clone groups have no P1 caller) |
| `syntax` plan | Opus | 361,458 | 30.8 min | Measured: grammar archive 4.76 MB vs 261.8 MB raw; cold build of 22 grammars 5.8 s, 1.6 GiB peak; all 30 contract queries compile |
| `prompts` crate and `cargo xtask generate` | Opus | 413,784 | 15.7 min | 128 templates byte-equal to Python; 33 new tests; clone share 0.0061 |
| `app`, `render` and `cli`, `hook` scoping | Sonnet Explore | 304,632 | 2.4 min (parallel) | `capabilities` prints fields with no Rust value in P1; the `op!` macro has no P1 caller beyond two operations |
| `/optimize` on `prompts` | Opus | 134,248 | 5.2 min | 1 LOW deferred to the P1 profile (presize `fill`, 50-215 ns per render); 1 rejected; render already 3x faster than Python `str.format` |
| `git` crate | Opus | 479,246 | about 2 h with the usage-limit stop | 59 tests; differential 0 differing lines on 2 repositories; clone share 0.0117 |
| `store` plan | Opus | about 324,000 | 27 min | Read-only connection with a 5 s busy timeout gives Python's receipt text in 8 of 9 measured states; the planner re-sent its plan 3 times and was replaced by a fresh executor |
| `app` plan | Opus | about 333,000 | 2 sessions | `op!` deferred to P2; capabilities equal to Python except 2 fields |
| `metrics` crate | Opus | 396,118 | 2 sessions (parked while `syntax` built) | Corpus: 0 differences on 761 files (13,031 units, 16,901 blocks) and on the same files cut to half length |
| `syntax` plan and crate | Opus | 690,340 | 2 sessions | Corpus: 0 differences on 943 files (2,871,052 nodes); the PM check found a stale generator output (py-tree-sitter capture order varies per run), fixed by a sort |
| `io` crate and `pycompat` additions | Opus | 835,587 | 14.9 min (last session) and 2 earlier sessions | 45 `io` tests, 29 `pycompat` tests; 84 config documents equal to CPython; clone share 0.0090; also took the requests of 3 other crates (store, app, checks) |
| `checks` crate | Opus | 487,305 | 26.5 min (last session) and earlier sessions | 1,103 fixture cases; corpus differential 0 differences in 300 cases (33,102 pair runs) over 2 repositories |
| `store` build | Opus | 483,793 | 35.8 min | 16 tests from the Python reader's fixtures; brought in `foldhash`, whose license exception another executor had removed |
| `app` build | Opus | about 420,000 (753,136 with the plan) | 3 sessions | 35 tests; end-to-end differential 0 differences in 128 runs; the PM check found a fixture that held the executor's scratch path |
| `render` build | Opus | 415,133 | 27.6 min | Both lint goldens byte-exact; started from the approved `app` API before `app` finished |
| `hook` plan | Opus | 275,548 | 17.4 min | 35 measured config cases: modes equal in all, log warning texts equal in 10 (deviation D2) |
| `cli` plan | Opus | 234,725 | 12.7 min | clap matched 0 of 12 captured help texts; a hand-written argparse subset chosen for byte parity |
| `cli` build | Opus | 618,289 in the last of 3 runs (the earlier runs are not in this figure) | 3 runs, the last 11.0 min | 30 tests in the PM run; full-binary differential 394 of 394 equal; its first `main` held the stream locks across the hook call, which hung the hook's SIGTERM path; the hook executor found it and `cli` fixed it |
| `hook` build | Opus | 873,772 in the last of 3 runs (the earlier runs are not in this figure) | 3 runs, the last 40.6 min | 517 workspace tests in the PM run; hook differential 944 calls with 0 differences; found the stream-lock hang and the unreadable-cwd difference, both fixed without a new deviation |
| P1 verification | Opus | 210,814 | 28.1 min | Every gate, 12 generators twice, cli 394 of 394 and hook 944 of 944 equal; found 1 blocker (a gitignored source file) and 3 smaller items |
| P1 verification fixes | Opus | 180,998 (2 runs) | 2 runs, 42.5 min in all | Module rename, one format fix, 5 self-test rows; self-test 177 of 177 rows from a clean clone of a scratch commit |
| `pycompat` split by domain | Opus | 254,883 plan + 754,344 build | 17.3 min plan, 56.7 min build | 104 files; 516 tests (the `max_first` test left with its function); fixtures byte-equal; 18 doc lines; the `pystr` module rename followed |
| `/optimize` reviews A, B, C | Opus | 410,113 + 363,890 + 378,018 | 24.7, 30.7 and 27.2 min, in parallel | 3 hot paths profiled; 20 distinct findings after merging duplicates, 14 applied, 6 rejected (a parity hazard, a lost self-check, 4 low gains) |
| `/optimize` builds O1, O2, O3 | Opus | 501,097 + 369,388 + 319,364 | 74.3, 28.5 and 27.4 min, in parallel | `lint --since HEAD~1` 256 to 129 ms; hook fresh 39.6 to 28.6 ms; output equal in every A/B run |
