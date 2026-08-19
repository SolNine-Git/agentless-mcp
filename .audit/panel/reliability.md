# Panel Review — Reliability Engineer

Reviewed: 2026-08-19
Scope: operational resilience across all 24 audited blocks (error handling, observability,
resource/timeout discipline, config robustness, SPOF risk on the SQLite index cache).
Method: read `.audit/progress.md` and all 24 `.audit/findings/B*.md` files in full; spot-checked
source line references cited below against the repository at HEAD. No new empirical probes were
run — the findings restate and re-weight measurements already made by the per-block reviewers,
scored specifically against operational-resilience criteria rather than correctness.

System framing: this is a long-lived MCP server plus a CLI, both fronting a single SQLite index
cache, both able to spawn subprocesses (git, grammar downloads, arbitrary repository-supplied test
commands in git worktrees). The baseline has 1140 passing tests and no CI — every "enforced"
guarantee in this report is enforced only when a contributor's local hooks ran.

---

finding_id: 1
persona: RELIABILITY_ENGINEER
severity: CRITICAL
scope: system
files: [src/agentless_mcp/core/cache.py]
category: resilience
issue: CachedSource decides freshness from an `entries` snapshot taken once at open time, then reads `tags`/`imports`/`refs` in separate autocommit SELECTs with no BEGIN pinning a single WAL snapshot across the two. If a second `agentless-mcp index` run prunes a file's rows between the freshness check and the row read, the freshness gate still says "fresh" and the row read returns zero rows — a real cache miss disguised as "this file defines nothing," which is a value the design also uses legitimately (a file with zero symbols). Reproduced end to end against a real database: `find_referencing_symbols` answers "no callers" for a symbol that has callers while the MCP server stays up during a concurrent `index` run.
evidence: core/cache.py:320-368 (_fresh_digest, _symbol_rows), core/cache.py:486-493 (open_source). Module docstring at cache.py:22-30 states "cannot serve a wrong answer" — the exact guarantee this defect breaks. Reproduced by the block reviewer with a live extractor and database (B08-C1 in .audit/findings/B08.md).
source: read skew under snapshot isolation (Designing Data-Intensive Applications, ch. 7, "Weak Isolation Levels" — two reads inside one logical operation observing two different points in time); CLAUDE.md "Errors surface" (a miss must be handled as a miss, not coalesced into a plausible answer).
recommendation: Open the read connection with an explicit BEGIN held for the CachedSource's lifetime (sources are per-request, so the scope is natural) so every row query sees one WAL snapshot consistent with the freshness decision. Failing that, treat a zero-row result as an unconfirmed miss and fall through to on-demand extraction rather than returning it as an answer.
confidence: HIGH

finding_id: 2
persona: RELIABILITY_ENGINEER
severity: CRITICAL
scope: system
files: [src/agentless_mcp/application/envelope.py, src/agentless_mcp/core/projectconfig.py]
category: config
issue: The response header (receipt lines, including one line per unknown key in `.agentless-mcp.json`) is assembled outside the token budget and only *subtracted* from it afterward; `projectconfig.py` caps `stoplist` entries but applies no cap to the unknown-key warning list. A `.agentless-mcp.json` with ~8,000 short unknown keys (well under the 64KB config-file cap) drives the header alone past 300,000 tokens against a `max_tokens=16000` call, and `_fit` returns an empty body — every tool response for that repository becomes ~19x over budget and carries zero lines of the actual answer, forever, until the config file is edited. This is a functional denial-of-service triggered purely by repository content the tool was asked to read, on every single read tool.
evidence: application/envelope.py:127,136,145-148 (header assembled then subtracted from budget, no cap on the subtraction term); core/projectconfig.py:201-205 (unbounded `unknown` key loop, contrasted with the capped `stoplist` at :278-282). Measured by the block reviewer: 8,000 keys -> 301,789 actual tokens against a 16,000 ceiling, 100/100 body lines dropped (B16-C1 / cross-referenced at B04-H2 in .audit/findings/B16.md, B04.md).
source: SRE book, "Closing Remarks" on overload — "When systems are overloaded, something needs to give"; here nothing sheds load at the one place a bound is documented to exist (envelope.py:31-33's own "hard 16k-token cap" claim).
recommendation: Cap `ProjectConfig.warnings` at a small fixed count in `projectconfig.parse` (mirroring `MAX_STOPLIST_ENTRIES`) and additionally clamp the whole header before it enters the budget computation (e.g. `_fit(header, counter, max_tokens // 4)`), so no code path can emit more than `max_tokens` regardless of what future receipt fields are added.
confidence: HIGH

finding_id: 3
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: system
files: [src/agentless_mcp/core/cache.py]
category: resources
issue: `_discard` (called from the *read* path when a cache looks unusable) unlinks the database, `-wal`, and `-shm` files with no lock, while `_open_for_write` creates the schema in autocommit before `_plan_index` runs — so from the moment a write starts until it commits, a concurrent reader sees an empty `meta` table, decides the cache is unusable, and deletes it out from under the in-progress writer. `unlink`-ing an active WAL file is undefined behavior in SQLite; the milder measured outcome is a writer that finishes, prints a success line, and has written zero rows because its own committed WAL frames were deleted mid-run. The module's own docstring claims "Readers never block" and states this exact scenario cannot happen.
evidence: core/cache.py:461-481 (_discard, no lock taken), :867-882 (_open_for_write creates schema in autocommit before rows exist), :1094-1103. Reproduced by the block reviewer: `db exists after reader: False`, `writer committed rows: _RowCounts(files=0, tags=0, imports=0, refs=0)` (B08-H1 in .audit/findings/B08.md).
source: CLAUDE.md "One owner per state change" — build_index mutates the cache under write.lock, open_source mutates it via _discard without; "Errors surface" — success is reported for a run that wrote nothing.
recommendation: Route `_discard` through the same `write.lock` every other mutation uses, and do not leave a schema-only database visible for the run's duration — write the meta row inside the same BEGIN IMMEDIATE as a sentinel readers recognize as "build in progress," or build into a sibling file and rename over the target under the lock.
confidence: HIGH

finding_id: 4
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: module
files: [src/agentless_mcp/core/cache.py]
category: error-handling
issue: `open_source`'s docstring and the CLI's own comment both promise "a read command never fails because of a cache" — every failure degrades to on-demand parsing. Two paths violate it: `_discard`'s `Path.unlink(missing_ok=True)` swallows only `FileNotFoundError`, so a `PermissionError` (read-only cache dir, or a file another process has open on Windows — a platform this package explicitly supports) propagates unhandled out of `open_source`. Separately, for a non-git repository that grew past its walk bound since indexing, `repo_generation`'s internal `walk_repo` call is unguarded and raises `WalkBoundExceeded` from what is meant to be a cost signal only — so a `read_slice` call that would otherwise not walk the tree at all fails, but only once an index database happens to exist, meaning building an index turns a previously-working command into a failing one.
evidence: core/cache.py:449-455, :1053-1063, :1094-1103; contract stated at adapters/cli/main.py:1300-1307 ("Every failure to open one degrades to on-demand parsing ... so a read command never fails because of a cache") (B08-H3 in .audit/findings/B08.md).
source: CLAUDE.md "Errors surface" — a documented degrade-on-failure contract must actually catch the failures it claims to degrade.
recommendation: Wrap `_discard` in `except OSError` and fold the reason into the note; wrap the `repo_generation` call inside `open_source` so a walk-bound or OS failure yields `repo_generation=None` with a note rather than propagating — the generation is advisory, never correctness-bearing, and must not be able to fail a read.
confidence: HIGH

finding_id: 5
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: module
files: [src/agentless_mcp/adapters/mcp/server.py]
category: resilience
issue: `context.list_roots()` is an out-of-process JSON-RPC round trip back to the connected client, awaited on the critical path of every one of the eleven published tools via `context_for`, with no `asyncio.timeout`/`wait_for` and no cancellation. A client that advertises the `roots` capability at initialize but never answers `roots/list` (a documented real-world MCP client bug class) hangs every single tool call on this server forever, with no other work available to the process and no symptom beyond "the tool never returns." The `except McpError` handler is also narrower than the failure set — a payload that fails pydantic validation raises `ValidationError`, and a dying transport raises an unrelated `anyio` exception, neither of which is caught.
evidence: adapters/mcp/server.py:451, 483, 657 (B22-H3 in .audit/findings/B22.md).
source: "Every network or out-of-process call gets a timeout" (CLAUDE.md, Engineering Invariants); Designing Data-Intensive Applications ch. 8, "Timeouts and Unbounded Delays" — a call with no timeout has no upper bound on how long a fault can block correct operation.
recommendation: Wrap the await in `asyncio.timeout(...)` with a sub-second budget and treat expiry identically to `McpError` (log, fall back to static roots). Broaden the except to cover validation and transport failures with the same fallback, and consider caching the answer per session instead of re-querying on every call.
confidence: HIGH

finding_id: 6
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: system
files: [src/agentless_mcp/core/sandbox.py]
category: resources
issue: The `try/finally` that releases a git worktree starts *after* `git worktree add` runs, so every failure of the add itself — the 120s timeout expiring on a large checkout, a killed process, a full disk — leaves a locked worktree record with no cleanup. `git worktree prune` (the only cleanup mechanism) explicitly skips locked records, so the phantom entry in `.git/worktrees` is permanent, and the multi-hundred-MB partial checkout under `$XDG_CACHE_HOME/agentless-mcp/worktrees` is never swept. There is no scratch/cache GC command anywhere in `src/` — the only removal path is the per-run `_release`, so every skipped release (a SIGKILLed CLI, this failure mode, or any of the other unhandled-exception paths documented across other blocks) strands disk space permanently. README states "no cache or scratch state is written inside the repository under analysis," which this scenario falsifies for the worktree record and for the lifetime of a normal `patch apply`.
evidence: core/sandbox.py:184-191 (try starts after `worktree add`). Reproduced: killing a 766 MB `git worktree add` mid-checkout leaves a permanently locked record even after `git worktree prune` (B14-H1 in .audit/findings/B14.md); cross-block note confirms no GC exists anywhere in src/.
source: CLAUDE.md "External calls fail, hang, or duplicate ... what happens when the dependency is down is a design decision, not an afterthought" — here the failure path was simply not designed.
recommendation: Wrap the add in its own try/except and on any failure run `worktree remove --force` (which does handle a locked record) plus `shutil.rmtree` before re-raising, so creation and release share one cleanup scope. Separately, add a scratch-directory GC (age-based sweep of `worktrees/`) as a first-class operational command — nothing in the codebase currently owns this.
confidence: HIGH

finding_id: 7
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: module
files: [src/agentless_mcp/core/sandbox.py]
category: resources
issue: The module's own comment frames `DEFAULT_MAX_CAPTURE = 100_000` as "the cap on captured output," but it only bounds what is read back afterward — the child process writes into an unbounded `tempfile.TemporaryFile` for the entire `timeout` window (default 300s, and the CLI enforces no ceiling on `--timeout` beyond `> 0`). On systems where `/tmp` is tmpfs (common), a runaway test-suite print loop or a binary dump to stdout during a `validate --jobs N` run consumes RAM at write speed, multiplied by `2N` concurrent streams, until the time bound expires — potentially taking down unrelated processes on the machine with no diagnostic beyond a timeout report.
evidence: core/sandbox.py:87-89, 244-255, 364-380; no RLIMIT_FSIZE or truncation on the write side (B14-H2 in .audit/findings/B14.md).
source: CLAUDE.md "External calls fail, hang, or duplicate. Every network or out-of-process call gets a timeout" — the time bound exists here, but the companion resource bound (size) does not, which is the half SRE literature treats as equally load-bearing for subprocess sandboxing.
recommendation: Bound the writer, not just the reader — set RLIMIT_FSIZE on the child via preexec_fn, or have the wait loop stop the process once the temp file passes a small multiple of max_capture. Fix the misleading comment either way.
confidence: HIGH

finding_id: 8
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: system
files: [src/agentless_mcp/core/extractor.py, src/agentless_mcp/core/cache.py]
category: resilience
issue: Four tree-walking functions in the extractor recurse per child with no explicit stack (the sibling `walk_nodes`/`collect_refs` are correctly iterative and the module docstring credits this as deliberate stack-safety). Measured thresholds are ordinary, not adversarial: 248 chained JS method calls or 496 concatenated string terms trip `RecursionError`, a shape routine in minified bundles and generated API clients. Because `cache._plan_index`'s per-file try/except catches only `LanguageUnavailable`, the `RecursionError` is not contained to the offending file — it propagates out of the whole index-build loop, so one qualifying file anywhere in a repository aborts `agentless index` (and therefore every downstream read tool) for the entire repository, with a raw Python traceback rather than a per-file degradation. `core/patchlint.py` already enumerates `RecursionError` as an expected, containable failure mode in its own DEGRADED_ERRORS tuple — the index scan was never given the same treatment.
evidence: core/extractor.py:782-854, 957-980, 1337-1362, 1379-1398 (recursive walkers); core/cache.py:657-663 (except narrowed to LanguageUnavailable only). Measured by the block reviewer via binary search on real grammars (B07-C1 in .audit/findings/B07.md).
source: CLAUDE.md "Errors surface ... handle it meaningfully or let it propagate" combined with the absence of a bulkhead per unit of work — one file's parse failure should not be a whole-repository outage (SRE book, "Closing Remarks" on isolating failure domains within a system).
recommendation: Convert the four recursive walkers to the explicit-stack form `walk_nodes` already uses. Independently and immediately, widen `cache._plan_index`'s except to the same tuple `patchlint.DEGRADED_ERRORS` already uses (including RecursionError), so one pathological file degrades to an IndexFailure row instead of failing the whole index.
confidence: HIGH

finding_id: 9
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: module
files: [src/agentless_mcp/application/patch_service.py]
category: error-handling
issue: The write loop that applies edits to disk (`--in-place` or a scratch worktree) has no exception handling and writes files one at a time from a dict with undefined iteration-failure semantics. Two measured partial-failure shapes: (1) when one of several edits in a patch fails to apply, `new_contents` still holds every file a *successful* sibling edit touched, so those are written while the failed file's target is untouched — the checkout now holds half a patch with `report.ok = False` and nothing telling the caller which half; (2) a mid-loop `OSError` (e.g. a read-only file second in write order) propagates as a raw, uncaught exception — `run()` catches only `AtlasError`, so the CLI dies with a Python traceback, no `ApplyReport`, and no record of which files were already written before the failure. This same write path is reused by `validate_service` to write into the worktrees candidates are judged in, so damage here can fail a candidate for corruption the tool itself introduced.
evidence: application/patch_service.py:342-343 (unconditional write loop, no try/except); measured PermissionError mid-loop leaves a.py written and z.py not (B20-H3 in .audit/findings/B20.md); B20-C1 in the same file separately documents the write path using `read_bounded`'s lossy `errors="replace"` decode, so a non-UTF-8 file is silently corrupted outside the patch's own edited region and still reports `ok=True` — the same write-path robustness gap from the encoding angle.
source: CLAUDE.md "Errors surface" and "Functional core, imperative shell — fetch state, decide, then persist; never interleave reads/decisions/writes with no rollback path."
recommendation: Decide and state the invariant: either refuse to write anything when `not result.ok` in in_place mode, or explicitly document partial-write as expected and have the receipt print a loud "working tree now holds a partial patch; git checkout -- to restore" line. Wrap the write loop so an OSError becomes a typed AtlasError naming both the failing path and every path already written, and switch to atomic write-then-rename (os.replace) so an interrupt cannot leave a truncated file.
confidence: HIGH

finding_id: 10
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: module
files: [src/agentless_mcp/application/validate_service.py, src/agentless_mcp/core/vote.py]
category: error-handling
issue: `RunStatus.ERROR` means the regression-test command never started (spawn failure — EAGAIN under high --jobs, ENOMEM, a patch that renamed the test runner). `Verdict.of` maps this straight to `Verdict.ERROR`, indistinguishable downstream from "the tests ran and failed." `_vote_candidate` reduces it to `regression_passed=False`, so a candidate whose regression suite never executed is ranked in the vote's "applied" tier exactly as if its tests had genuinely failed — a run where every candidate's regression command errored produces a confident, named winner and a report that reads "here is the best of what did not break anything" when literally nothing was measured. The module is otherwise careful about exactly this distinction at the baseline level (`Verdict.not_evaluated` is documented as "deliberately not failed: nothing was measured, and a report that says otherwise is inventing evidence") — the care does not extend to the candidate run step, and `ValidateReport.warnings()` has no branch for it at all.
evidence: application/validate_service.py:491-509 (Verdict.of mapping ERROR with no distinct downstream handling), :292-294, :310-344 (warnings() has four branches, all baseline-side, none for candidate-level ERROR). Measured: three candidates all "regression=error" (nothing ran) still produce tier `applied` with a crowned winner (B21-H1 in .audit/findings/B21.md).
source: CLAUDE.md "Errors surface" — infrastructure failure must not be laundered into a domain verdict; this is the textbook "silent data corruption" case for a decision pipeline, where the corrupted value is a ranking rather than a row.
recommendation: Count Verdict.ERROR (and TIMEOUT) candidates separately in ValidateReport and emit a warning naming them ("N of M candidates were never measured: the test command could not be started"), and have _vote_candidate exclude an errored candidate from the applied-tier ladder the way an unapplied one is already excluded, rather than ranking it as if it had run.
confidence: HIGH

finding_id: 11
persona: RELIABILITY_ENGINEER
severity: HIGH
scope: module
files: [src/agentless_mcp/application/validate_service.py]
category: error-handling
issue: `load_verdicts` is documented as "strict on purpose, the only reader of this format ... a document whose repro_valid went missing must not read as False and quietly demote the ladder by one rung." That strictness is real for `repro_valid` (typed bool) and `baseline` (coerced through an enum), but the three fields that actually decide each candidate's rung — `apply.status`, `regression`, `reproduction` — are compared with bare string equality against the expected enum value, with no validation that the value is a recognized member at all. A future rename of an enum spelling (or a verdicts file from a mismatched tool version) silently turns every winning candidate into a losing one with zero errors surfaced anywhere: the ladder falls to "applied" or "none," the CLI exits 1, and the run simply reports that nothing worked.
evidence: application/validate_service.py:620-627 vs. the strictness claim at :571-576; measured: `regression="pased"` (typo) silently becomes `regression_passed=False` with no error (B21-H2 in .audit/findings/B21.md). Same block flags a related boundary crack: `BaselineStatus(_string(...))` at :595 raises a bare `ValueError` instead of the module's own `AtlasError`, and `run()` only catches `AtlasError`, so a hand-edited verdicts file can crash the CLI with a traceback instead of a diagnosed refusal.
source: CLAUDE.md "Boundary integrity — foreign data crosses into the system through one parse step that converts it to typed domain values or raises. No coalesced defaults ... a renamed field must fail at the boundary, not read downstream as a plausible 'no data yet'."
recommendation: Parse these three fields into their enums (Verdict(...), ApplyStatus(...)) inside a try that raises a typed AtlasError naming the field, the bad value, and the line number, and derive every boolean from the typed value rather than string comparison. Wrap the BaselineStatus coercion in the same helper.
confidence: HIGH

finding_id: 12
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/patchlint.py]
category: logging
issue: `DEGRADED_ERRORS` — the tuple of exception types each lint check is allowed to swallow into a "not_checked" gap line — includes `TypeError`, `KeyError`, `IndexError`, and `AttributeError`, which are exactly the classes a genuine bug inside this module's own code would raise. The comment above the tuple states the opposite intent explicitly: "Named explicitly instead of catching Exception: an error class not in this list is a defect in this module and must surface as one." As written, essentially any internal defect — a None dereference, an off-by-one, a renamed field on ASTSymbol — is caught by `_guarded` and rendered as a clean-looking `not checked: the check could not run over this patch (AttributeError)` line, with no traceback and no indication of where inside the check it failed; six of the module's seven checks still produce an apparently-healthy report around it. Neither the guard branch nor the fallback handler is exercised by any test.
evidence: core/patchlint.py:176-188 (DEGRADED_ERRORS definition and its own contradicting comment), :662-674 (_guarded), :707-708 (_fragment's handler) — both paths uncovered by the full suite (B15-M3 in .audit/findings/B15.md).
source: CLAUDE.md "Errors surface ... When logging a failure, log the context — ids, inputs, the why — not just the event."
recommendation: Narrow DEGRADED_ERRORS to what foreign input actually raises through this module's own boundaries (AtlasError, ValueError, RecursionError, OSError). If AttributeError/KeyError must stay to tolerate odd fragments from the extractor, log the traceback into the finding's evidence field so a real internal defect is diagnosable from the report rather than indistinguishable from an expected degradation.
confidence: MEDIUM

finding_id: 13
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: module
files: [src/agentless_mcp/bootstrap.py]
category: logging
issue: `mcp_main` catches any `ImportError` raised while importing the 684-line MCP server module (which itself transitively imports the whole application layer — 15+ first-party modules) and reports it unconditionally as "the MCP server needs the 'mcp' extra, which is not installed," with an install command. A genuine wiring failure — a renamed export in the application layer, a partially-broken third-party install unrelated to the mcp/tokens extras — produces the identical false diagnosis, sending the operator to reinstall an extra they already have while the real cause (a code regression) goes uninvestigated. The chained exception text is included parenthetically but framed inside a claim that is usually wrong.
evidence: src/agentless_mcp/bootstrap.py:132-140; verified with a probe raising a first-party ImportError through the same import path, which produces the same misleading message (B24-H1 in .audit/findings/B24.md). Compounding: the entire construction of ServerServices two lines later is untyped (ModuleType attribute access is Any to mypy) and executed by zero tests, so a required-field mismatch between bootstrap and the server module would pass every gate and only surface as a startup TypeError in production (B24-H2, same file).
source: CLAUDE.md "Guards key on the invariant, not a proxy" — the guard should test for the actual missing dependency (find_spec on fastmcp/mcp/pydantic), not "any ImportError from this import statement."
recommendation: Probe with importlib.util.find_spec for the extra's actual distributions before importing, and let any other ImportError propagate with its traceback. Add one test that invokes mcp_main with serve patched to a recorder, pinning ServerServices' constructor arity and serve's call shape so a drift fails at test time instead of at a user's process startup.
confidence: HIGH

finding_id: 14
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/grammars.py]
category: resilience
issue: The warmed-state guard (`warmed_languages()`, keyed on canonical names only) and the admission check (`pack.has_language`, which matches name-or-alias per the third-party library's own docs) operate over different universes. For an alias like "shell" (which resolves to the already-warmed "bash" grammar), `agentless-mcp warmup shell` fails, tells the operator to "run agentless-mcp warmup," and running that exact command fails identically forever — there is no input that terminates the loop. With the no-download flag set, the same alias trips a hard refusal ("refusing to download grammar 'shell'") for a grammar that needs no download at all. This is an operational dead-end: the tool's own remediation text does not resolve the condition it names.
evidence: core/grammars.py:181-187 vs :250-257; measured against the installed pack: `has_language('shell') == True`, `downloaded_languages()` excludes it, `warmup(['shell'])` fails with the exact "not warmed: run agentless-mcp warmup" message that caused the retry (B06-H2 in .audit/findings/B06.md).
source: CLAUDE.md "Guards key on the invariant, not a proxy" — the invariant is "is this grammar loadable without a network fetch," and the guard is instead a string-membership proxy that breaks on aliases.
recommendation: Resolve the caller's name to its canonical form once, at the boundary, and gate on the canonical name throughout — or make the warmed check probe loadability directly instead of comparing name sets. Reject an alias the module will not accept at argument-parsing time, naming the canonical spelling, instead of looping.
confidence: MEDIUM

finding_id: 15
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/grammars.py]
category: resilience
issue: Grammar-pack downloads (`pack.prefetch`) are the package's only network call, and neither the call site nor the third-party PackConfig it wraps exposes a timeout, connect-timeout, or retry budget of any kind — verified against the installed library's actual signature. A stalled or half-open connection (a captive portal, a proxy that accepts and never answers) hangs `agentless-mcp warmup` indefinitely with no output and no partial report; in CI this silently consumes the whole job timeout with a log that ends mid-run rather than a diagnosed failure.
evidence: core/grammars.py:258-268; `inspect.signature(pack.prefetch)` and `dataclasses.fields(PackConfig)` confirm no timeout knob exists anywhere in the dependency (B06-M3 in .audit/findings/B06.md). Same theme recurs at bootstrap.py:67-77 (B24-M1): `TiktokenCounter.__init__` calls `tiktoken.get_encoding`, which performs an untimed `requests.get` on a cold cache, and the exception it raises on a refused connection (`requests.exceptions.ConnectionError`) is not an AtlasError, so it escapes the module's own error guard and reaches the user as a raw traceback from the composition root instead of the documented "actionable message instead of a traceback."
source: Designing Data-Intensive Applications, ch. 8, "Timeouts and Unbounded Delays" — a call with no timeout has no upper bound on how long a fault can block correct operation; CLAUDE.md "Every network or out-of-process call gets a timeout."
recommendation: Run prefetch under an explicit deadline at the call site (worker thread or subprocess with a wall-clock bound) since the dependency offers no knob, turning expiry into the same degraded row a pack.Error already produces. Wrap tiktoken's get_encoding in the same try/except that already guards its import, converting any exception into a typed AtlasError naming the encoding and pointing at TIKTOKEN_CACHE_DIR for offline pre-seeding.
confidence: HIGH

finding_id: 16
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/cache.py]
category: resources
issue: `FileSource` (the protocol both `CachedSource` and `OnDemandSource` implement) has no `close()` in its interface, even though `CachedSource.close()` exists concretely. Neither adapter calls it — grep across `src/` finds exactly one `.close()` call in the whole codebase, in `util/filelock.py`. The test suite's own output shows the cost: the majority of 36 warnings on a 52-test run are `ResourceWarning: unclosed database`. Because the protocol does not declare close, a caller holding a `FileSource` cannot release it without an isinstance check first, so the design makes the correct call structurally awkward even for a caller that wants to do it right. Safe today only because CPython's refcounting collects the connection when the per-request RepoContext dies; a deferred-GC runtime would accumulate open SQLite connections under sustained MCP server load, and the warnings currently mask any real ResourceWarning that might appear.
evidence: core/cache.py:159-186, :316-318; test output "52 passed, 36 warnings" (B08-M5 in .audit/findings/B08.md).
source: CLAUDE.md architecture rule "Deep modules ... a public interface should be far smaller than the implementation it hides" applied to resource lifecycle — the protocol should expose the operation every implementation needs, and does not.
recommendation: Add close() to the FileSource protocol (a no-op on OnDemandSource) and have both adapters close the source when the request ends — contextlib.closing around the per-request source-open call in both the MCP server and the CLI.
confidence: MEDIUM

finding_id: 17
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/sandbox.py]
category: resources
issue: The module's own comment names the exact race it then fails to prevent: "`git worktree add` and `git worktree prune` race each other: prune walks the repository's worktree records and can remove one that a concurrent add has created but not yet populated," and guards it with `threading.Lock`. A process-local lock provides no protection between two separate `agentless-mcp validate` invocations against the same repository, or between the CLI and anything else using this module concurrently — exactly the multi-process case the comment describes as the reason the lock exists. Under that race, one process's `_release` prune can drop another's worktree record while it is still being populated, leaving the second process with a working directory git no longer tracks and a subsequent `worktree remove` that fails.
evidence: core/sandbox.py:92-97, 419-430 (B14-M1 in .audit/findings/B14.md); the same file also runs `git worktree add` without neutralizing the analysed repository's hooks/config (unlike the sibling `diff()` call twelve lines away, which explicitly does), so worktree creation on an untrusted checkout can execute arbitrary code before any test command runs (B14-M2) — noted here because it is also a resilience/blast-radius concern for the sandbox that is supposed to isolate exactly this.
source: CLAUDE.md "Guards key on the invariant, not a proxy" — the invariant is exclusive access to the repository's worktree bookkeeping across every process that might touch it; a per-process mutex is a proxy that only holds within one process.
recommendation: Reach for the repository-scoped util/filelock primitive the tag cache already uses instead of threading.Lock, and run prune only on the fallback branch where a record is actually known-stale rather than in the common release path.
confidence: MEDIUM

finding_id: 18
persona: RELIABILITY_ENGINEER
severity: MEDIUM
scope: cross-module
files: [src/agentless_mcp/util/errors.py, src/agentless_mcp/adapters/cli/main.py, src/agentless_mcp/core/treewalk.py, src/agentless_mcp/core/gitinfo.py]
category: error-handling
issue: The CLI's single top-level error boundary (`run()`) catches only `AtlasError`. Multiple blocks independently document the same consequence: an untyped `ValueError`, `OSError`, or `UnicodeDecodeError` raised anywhere below it reaches the user as a raw Python traceback instead of a diagnosed refusal with an exit code the caller can script against. Concretely: `contained_path` raises a bare `ValueError` for a NUL-containing path (B01-M1); `treewalk._git_listed_paths` catches `FileNotFoundError`/`TimeoutExpired` but not general `OSError` (B04-M4); `Path.read_text(encoding="utf-8")` raises `UnicodeDecodeError` (a ValueError subclass, not OSError) at three CLI sites that only catch OSError, producing a traceback whose accidental exit code (1) collides with the documented meaning of `EXIT_DOMAIN` ("the repository did not allow it") for at least one of those call sites (`diagram --check`), so a CI gate cannot tell "the diagram drifted" from "the file I pointed at is not text" (B23-H3). This is the same shape recurring at the boundary from three independent directions rather than three unrelated bugs.
evidence: util/errors.py + adapters/cli/main.py:129 (sole AtlasError catch); core/fslimits.py:41 (B01-M1); core/treewalk.py:136-149 (B04-M4); adapters/cli/main.py:877-880, 1054-1057, 1197-1202 (B23-H3). Also core/gitinfo.py:58-64 — git_root collapses four distinct degradation reasons (not a repo / git missing / timed out / git errored) into a bare None, so on a machine without git on PATH the CLI tells the operator "this is not a git repository" for a repository that is one (B04-M2), which is a diagnosability failure of the same family — a caller that cannot distinguish failure causes cannot act on the right one.
source: CLAUDE.md "Errors surface. Never swallow an error: handle it meaningfully or let it propagate ... When logging a failure, log the context." A typed error hierarchy exists in this codebase specifically to make this distinction possible, and the boundary that is supposed to consume it does not cover its actual raise-surface.
recommendation: Audit every module's actual raise-surface against the typed hierarchy the CLI boundary handles, and either narrow each raise site to a typed error or widen the boundary's except clause with an explicit, reasoned list (the way core/patchlint.DEGRADED_ERRORS attempts, imperfectly, for lint checks). Fix git_root to return its degradation note rather than discarding it, since both CLI callers already have code paths waiting to surface a specific reason.
confidence: MEDIUM

finding_id: 19
persona: RELIABILITY_ENGINEER
severity: LOW
scope: module
files: [src/agentless_mcp/application/validate_service.py, src/agentless_mcp/adapters/cli/main.py]
category: resilience
issue: `--timeout` bounds a single command; nothing bounds the run as a whole. A validate invocation costs up to `repeat_baseline + 1 + candidates * 2` full command executions, and `_validate_bounds` only checks that each flag is individually positive — there is no ceiling on `--jobs` (each job holds a full worktree checkout, so peak disk use is `jobs * repo_size`) and none on `--repeat-baseline`. With the default 300s timeout and 20 candidates plus a reproduction command, the documented worst case is roughly 200 minutes of wall clock with no progress output and no overall deadline — an operator or CI job has no way to know whether the run is proceeding normally or stuck partway through candidate 3 of 20.
evidence: application/validate_service.py:393-397, :434-436; adapters/cli/main.py:1398-1406 (checks only `> 0`) (B21-L1 in .audit/findings/B21.md).
source: SRE book discussion of overload and graceful degradation — an operation with no aggregate deadline and no progress signal cannot be distinguished from a hang by anything watching it externally.
recommendation: Cap --jobs at something like os.cpu_count() with a note when a request is clamped, and either offer a whole-run budget flag or state the worst-case wall clock in --help so operators can plan around it.
confidence: MEDIUM

finding_id: 20
persona: RELIABILITY_ENGINEER
severity: LOW
scope: module
files: [tests/unit/test_cache.py]
category: resources
issue: `test_cache.py` imports `fcntl` at module scope with no `sys.platform` guard or `pytest.importorskip`. `fcntl` does not exist on Windows, so the entire 684-line, 52-test file — including all freshness and equivalence tests that have nothing to do with file locking — fails at collection on Windows. The package deliberately supports Windows (`util/platforms`, `util/filelock`'s split POSIX/Windows implementation exist specifically for it), but the test suite that would prove the cache module works there cannot even be collected, so the documented cross-platform guarantee for the block with the most operational risk (the SQLite index cache) is unverified on the platform it claims to support.
evidence: tests/unit/test_cache.py:14 (B08-M6 in .audit/findings/B08.md).
source: CLAUDE.md "Hermetic tests" combined with the observability principle that a claimed guarantee needs a gate that actually runs; a guarantee that cannot execute on the target platform is not evidence about that platform.
recommendation: Move `import fcntl` inside the class that needs it, guarded by `pytest.importorskip("fcntl")`, so the rest of the module's tests still collect and run on Windows.
confidence: MEDIUM
