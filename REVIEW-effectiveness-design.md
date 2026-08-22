# Effectiveness + Design Review: `agentless-mcp` v0.3.1

- Review date: 2026-08-22
- Assessed revision: `ee439cf` (two commits past the assessed `02c15e20`: `5339bf3` agent-guide-as-data + unified-diff lint, `ee439cf` HTTP transport + operator-editable roots)
- Environment: Linux, Python 3.13.11 via `uv`
- Repos used: `agentless-mcp` itself (121 source files, 40,140 LOC) plus small synthetic fixtures in `/tmp/agentless-review`. Large SWE-bench repos (ruff, druid) were **not** exercised, per instruction; large-repo performance statements are labeled `assumption`.

## 1. Executive summary

**Verdict: effective, and its core invariants actually hold under measurement.** On the 23k-line self-repo, every headline capability worked as documented: map/skeleton/expand/find-symbol/refs/explain/path/cycles/communities/diagram all returned correct, tier-labeled, budgeted, byte-reproducible answers (cold map 1.275s → warm 0.597s; repeated calls byte-identical), the read-only and path-containment gates refused every escape tried (symlink, traversal, non-loopback HTTP), and the CLI-only patch pipeline correctly classified fix/wrong/no-op candidates in isolated worktrees with the checkout provably untouched. The 8 design invariants are enforced by real gates (import-linter 2/2 KEPT, deptry clean, 1562 tests passing in 23.33s), not by documentation.

The three most important things to change, in order:

1. **Degraded scans read as affirmative absence.** A repo whose only file exceeds the 1MB cap answers "no ranked files: nothing in this repository parsed into symbols" in text, while its JSON carries the truth (`skipped: [huge.py: exceeds the per-file cap]`). `render_map` (render.py:999) takes only `files`, and `find_symbol`'s JSON drops `skipped` entirely — so the agent-facing surface structurally cannot report the skip. This is the existing M2 finding, now confirmed twice (grammar-unavailable and size-cap) with a fresh repro.
2. **`index` exits 0 on partial failure.** Measured: 21 file errors on this repo's own checkout (its `pyproject.toml`, CI yaml, prompt JSON — the tier-2 grammars aren't warmed), exit code 0. `_cmd_index` (main.py:1428) unconditionally returns `EXIT_OK`, so a CI or install gate cannot detect a half-built index.
3. **Zero-block patch text parses as success.** `parse_blocks` (patches.py:236-241) splits on `>>>>>>> REPLACE`; text containing none — including the canonical Agentless/SWE-bench `*** Begin Patch` dialect, which this project is named after — returns `edits: [], errors: []`. `validate` catches it downstream ("the candidate contains no edits"), but `patch parse` and `lint` (which printed "no findings") report a false green to an agent checking its own patch.

Everything else found is lower severity: no end-to-end transport test for stdio *or* the new HTTP mode, no parallel indexing, and a tier-2 grammar set that leaves a repo's own config files unindexable by default.

## 2. Effectiveness findings (measured)

| Capability | Status | Measured evidence |
|---|---|---|
| `map` | **works** | Cold 1.275s / warm 0.597s (2.1x cache win); focus seed `server.py` ranked it first at 0.3129; two runs byte-identical; auto-budget output 8,250 chars ≈ 2,060 tokens (chars/4, derived), inside the documented 2000–8000 band (`AUTO_BUDGET_MIN/MAX`, map_service.py:51-52) |
| `skeleton` / `get_symbols_overview` | **works** | Map output is the skeleton; overview used throughout navigation; `depth=999` refusal verified in prior assessment, schema caps unchanged |
| `expand` | **works** | Bad id `py:...nonexistent.py::Nope` returned under `unresolved:` with the OS reason, not a crash or silent drop |
| `find-symbol` | **works** | Cold 0.908s / warm 0.221s (4.1x); correct stable ids in 4 languages (`py:`, `js:`, `rs:`, `go:`) on the fixture; common-name queries list all definitions as cards |
| `refs` | **works** | 48 references to `build_server`: 1 same-file (`serve`), 45 resolved-via-import (tests) — tiers labeled per file group, matching ground truth |
| `refs --shared-callers` | **works** | Damped scores (0.443, 0.244), callers listed with file:line |
| `explain` | **works** | Tiered fan-out card: 18 refs split same-file (6) / resolved-via-import (11) with per-symbol ids |
| `path` | **works** | 1-hop same-file `serve→build_server`; 4-hop cross-file `serve→personalized_pagerank` with per-hop edge kind + tier; unknown target refused with "no symbol or file matches" |
| `cycles` | **works** | Fixture with a↔b and c→a: reported exactly `pk/a.py -> pk/b.py -> pk/a.py`, c excluded; self-repo: "no import cycles" (consistent with import-linter) |
| `communities` | **works** | 24 communities over 121 files, modularity 0.358; byte-identical across runs |
| `diagram` | **works** | 8 nodes + explicit `... 113 more modules` elision node, solid/dashed legend |
| `patch parse/check/lint/validate/vote` | **works** | See below |
| `capabilities` | **works** | Full receipt: 15 warmed grammars, 7 unavailable, caps, effective config, cache generation |
| `index` | **degraded** | Works, but 21 errors on this repo's own files and exit 0 (finding D2) |
| Multi-language | **works (15/22)** | py/js/rs/go parse correctly; csharp, hcl, json, scala, sql, toml, yaml unavailable in the default warm state |
| No-git repo | **works** | Receipt: `head: nogit`, `dirty: unknown files`, plus an explanatory `# note:` line |
| Empty repo | **works** | "no ranked files: nothing in this repository parsed into symbols" — true here |
| >1MB file | **degraded** | `at_end` exists in a 1,038,917-byte file; text says "no matching symbols" / "nothing parsed into symbols"; only map-JSON carries the skip reason (finding D1) |
| Symlink / traversal escape | **refused** | `path refused: resolved to /etc/passwd, which is outside the root ...` |
| HTTP transport | **works (untested e2e)** | Loopback default (`DEFAULT_HTTP_HOST = "127.0.0.1"`, server.py:141); non-loopback `--host` refused at startup with a security rationale (server.py:1189-1194). Binding logic tested; no live-client test (finding D5) |
| Patch pipeline | **works** | Baseline green + repro red → "reproduces"; 01-fix: apply ok / regression passed / reproduction passed; 02-wrong: reproduction failed; 03 (zero-edit): `apply: failed, reasons: ["the candidate contains no edits"]`; `vote` picked 01-fix at tier `regression+reproduction`; checkout `git status` clean and HEAD unchanged after the run |

**Contract honesty** (`prompts/*.json` vs behavior): every specific claim probed held — the 2000–8000 auto band, `read_slice`'s "requires non-empty lines unless whole_file=true" (refused with that exact message over MCP), `list_dir` "honouring gitignore" (`.venv` absent), `expand` "every requested id comes back" (bad ids come back as `unresolved`), `diagram`'s elision marker. One gap: the README (README.md:86) says candidates "can use SEARCH/REPLACE text" without naming the accepted dialect; the canonical `*** Begin Patch` SEARCH/REPLACE dialect silently parses to zero edits (finding D3).

## 3. Design flaws (ranked)

**D1 — Skipped files are invisible on the agent-facing surface (HIGH).**
`render_map` (application/render.py:999) takes only `files`, and `symbol_service` builds `find_symbol` results without the `skipped` field (measured: JSON is `{"total": 0, "matches": []}` for a repo whose only file was skipped). Only `MapResult.as_dict()` (map_service.py:98) preserves it. Invariant 3/6 (evidence surfaced, errors surface): an agent reads "nothing in this repository parsed into symbols" for a repo that contains unparseable-by-cap content, and will make a wrong localization or duplication decision. Measured repro in §2. *Fix:* thread `skipped` into the text renderers as a `# warning:` receipt line (the no-git note already proves the pattern) and into `find_symbol`'s JSON; add a cold-grammar and over-cap regression test through both adapters.

**D2 — `index` exits 0 with a partially built index (HIGH).**
`_cmd_index` (adapters/cli/main.py:1403-1428) prints up to `INDEX_FAILURE_LINES` errors and then `return EXIT_OK` unconditionally; measured 21 errors, exit 0. The JSON path (main.py:1416-1418) also returns `EXIT_OK` with errors in the payload. Invariant 6 (errors surface): a gate that runs `agentless-mcp index` after `warmup` cannot detect that half the repo is missing. No test pins the error case (tests/unit/test_cli.py:130-133 pins only `errors 0`). *Fix:* return a distinct `EXIT_ERROR` (or `EXIT_OK` only when `report.errors == 0`) and add the CLI test.

**D3 — `parse_blocks` accepts block-less text as success (HIGH).**
patches.py:236-241: `text.split(REPLACE_MARKER)` with no markers yields `segments[:-1] == []`, so the loop never runs and the result is `edits=[], errors=[]` — no error for "this text contains no SEARCH/REPLACE blocks at all". The docstring promises "reporting every block that is not one"; zero blocks is the unreported case. `validate` catches it (measured: "the candidate contains no edits"), but `patch parse`'s JSON contract and `lint` ("01-fix: no findings") report success. Invariant 6 (boundary integrity: foreign text parsed to typed values *or raises*). Concrete impact: an agent that emits the canonical Agentless `*** Begin Patch` dialect — the format this project's own paper citation uses — gets a false green from the two commands an agent would run to check its patch. *Fix:* when `len(segments) == 1`, return a `ParseResult` with an explicit error ("no <<<<<<< SEARCH / >>>>>>> REPLACE blocks found; if this is *** Begin Patch text, ...") rather than an empty success.

**D4 — Default warm state cannot index a repo's own config files (MEDIUM).**
Capabilities lists `json, toml, yaml, hcl, sql, scala, csharp` as unavailable (tier 2). Indexing this repo errors on its own `pyproject.toml`, `.github/workflows/ci.yml`, `.pre-commit-config.yaml`, and all four `prompts/*.json` (measured: 21 errors). The tiering is a defensible design, but the consequence is that the default install produces a permanently noisy `index` run and an index that silently omits the files an agent most often wants to read (CI, config). *Fix:* either warm json/toml/yaml by default (small grammars) or downgrade "known language, not warmed" from an error line to a receipt warning and let `index` exit reflect it per D2.

**D5 — No end-to-end transport test for stdio or the new HTTP mode (MEDIUM).**
tests/unit/test_mcp_server.py tests transport *arguments* (test_stdio_is_the_default_transport :887, binding tests :896-908) but no test spawns the console script or a live HTTP server and completes `initialize` (grep for `subprocess|Popen` in the file: no hits). Prior assessment M3 documented a stdio init hang that reproduced on a minimal FastMCP server (attributed to the dependency stack, not this repo); the HTTP mode added at HEAD (commit ee439cf) has no transport gate at all. Invariant 7 (hermetic tests) is not violated — the gap is coverage of a shipped, documented entry point. *Fix:* one pytest that starts `agentless-mcp-server` (both transports) as a subprocess, initializes, asserts the exact 11-tool set, makes one bounded call, and kills it; use a loopback port bound in the test for HTTP.

**D6 — `--timeout` is not a wall-clock bound (MEDIUM, carried from prior assessment).**
`sandbox.py:100` `TERM_GRACE_SECONDS = 5.0`; the cleanup path sends SIGTERM, waits the grace, then SIGKILL *unconditionally* (sandbox.py:412-434) and waits the grace again after SIGKILL. A 1s timeout can consume ~11s. The docs' measured 6.002s overrun stands; the code path was re-verified but the probe not re-run. Invariant 4 (principled, named caps): the cap is named but the *documented* semantics ("hard bound") are wrong. *Fix:* tighten the second post-SIGKILL wait, or document total worst-case = timeout + 2×grace in the `--run-timeout` math.

**D7 — No test pins zero-block parse or index error-exit (LOW).**
Consequence of D2/D3: the two behaviors found broken have no regression test, so the fixes above need their tests written alongside. (tests/unit/test_patches.py has no case for block-less text; test_cli.py:130-133 only pins `errors 0`.)

**D8 — No test asserts read operations leave the repo unmodified (LOW).**
The write side is gated (tests/unit/test_sandbox.py:62-69 compares `git status --porcelain` + `rev-parse HEAD` before/after, and a clean checkout was measured after a full validate run), but the read path has no equivalent assertion — it is true by construction (the only writer is the sandbox, and `html --cache-file` writes only under the XDG cache, tested at test_cli.py:338-348). Cheap to add: snapshot `git status --porcelain` + tree hash in a fixture repo, run every read command, assert equality.

Not findings, checked and clean: no `except Exception` or silent catches anywhere in `src/` (grep: zero hits); the single `.get(k, [])` (map_service.py:387) is over a dict keyed by the same `candidate.path` that built the iteration order — a typing artifact, not foreign-data coalescing; all set iteration that reaches output is `sorted()` first (communities.py:177, mermaid.py:195); `datetime.now` appears only in SQLite cache metadata (cache.py:811), never in tool output; git subprocesses get a 30s timeout (treewalk.py:34,155).

## 4. Improvement opportunities

**Internal (ranked by impact/effort):**

1. **Close the three error-surfacing gaps (D1+D2+D3). Effort S, impact high.** One session: thread `skipped` into text + find-symbol JSON, non-zero index exit, zero-block parse error. Unblocks trusting the tool in an unattended agent loop — today a degraded scan or a malformed patch can read as "nothing here" / "all good".
2. **End-to-end transport tests (D5). Effort S, impact medium.** Unblocks shipping the HTTP mode and the stdio console command as monitored production dependencies; also the gate that would have caught M3's stack regression.
3. **Parallel index build. Effort M, impact medium at scale.** `build_index` (cache.py:622) extracts sequentially under a write lock; `assumption` (large repos not measured per instruction) it becomes the dominant cost on 1000+ file repos, where prior art (codebase-memory-mcp) claims kernel-scale indexes in minutes via worker pools. The per-file `(path, sha256)` plan already makes extraction embarrassingly parallel; only the final transaction is serial.
4. **Warm json/toml/yaml by default (D4). Effort S, impact medium.** Small grammars; removes the permanent 21-error noise on this repo's own checkout.
5. **Dead-code detection as a first-class query. Effort S (data exists), impact low-medium.** `refs` already computes fan-in; "symbols with zero referencing symbols outside their own file" is a filter, not new machinery, and it is a headline tool of the closest competitor.

**External (prior art to adopt, see §5):**

1. **Parallel worker-pool indexing** — from codebase-memory-mcp (arXiv:2603.27277).
2. **SCIP-compatible export** — an optional `index --export-scip` gives Sourcegraph-style tooling interop and a durable, language-agnostic symbol format (scip-code.org). Effort L.
3. **tree-sitter queries / tree-sitter-tags for extraction** — `identifier_node_types` (extractor.py:782-788) is a hand-maintained per-language node-type table; the tags ecosystem provides per-language definition/reference captures that could replace part of the hand-rolled pass and add definition-vs-reference distinction for free. Effort L, but it retires a maintenance surface.

## 5. Prior art / competitive landscape

- **[codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp)** + [arXiv:2603.27277](https://arxiv.org/abs/2603.27277): persistent tree-sitter knowledge graph over MCP, 66–158 languages, parallel worker pools, Cypher queries, dead-code detection, impact analysis, 3D UI; claims 83% answer quality at 10x fewer tokens over file-by-file exploration (31 repos). **Adopt:** parallel indexing, dead-code query. **Note:** it writes to agent config files, runs a multi-client session daemon, and ships a native binary with a documented Defender false-positive — a different trust posture.
- **[SCIP](https://scip-code.org/) / [scip-code/scip](https://github.com/scip-code/scip/)**: Sourcegraph's language-agnostic protobuf index for go-to-definition/find-references/find-implementations. The standard durable format agentless's SQLite tag cache does not speak. **Adopt** as an export target for interop.
- **[tree-sitter queries](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/index.html) / [code navigation via tags](https://tree-sitter.github.io/tree-sitter/4-code-navigation.html)**: the query language and the `tags` command give per-language definition/reference extraction that Aider's repo map uses; agentless hand-rolls the node-type tables instead.
- **[tree-sitter-language-pack's own MCP server](https://docs.tree-sitter-language-pack.xberg.io/guides/mcp-server/)**: the exact dependency agentless pins (==1.14.3) ships an MCP server with parsing + "code intelligence" extraction. agentless re-implements extraction on top of the same pack — a deliberate divergence (evidence tiers, receipts, validation), but the upstream `process` extraction is worth a documented comparison to avoid drifting from its fix stream.
- **Aider's repo map** (already cited by the repo's own docs): the direct precedent for PageRank + token-budgeted maps; agentless adds the evidence tiers, receipts, and validation that Aider lacks.

**What agentless-mcp already does better than this prior art** (measured, not claimed):

1. **A read-only, daemon-free, auditable trust posture.** No writes to agent config, no background daemon, no native binary beyond the SHA-256-verified, version-pinned grammar bundles (docs/supply-chain-audit.md), non-loopback HTTP refused at startup (server.py:1189-1194), and every answer carries a repo/HEAD/cache-generation receipt. codebase-memory-mcp's own README leads with "writes to your agent configuration files"; agentless's MCP surface is exactly 11 read tools (counted from the live client; exact-set asserted in tests/unit/test_mcp_server.py).
2. **Evidence-tiered references with the weak tiers labeled, not promoted.** No MCP navigation tool found carries per-reference binding confidence; SCIP is ground truth but requires per-language semantic indexers. The four tiers (same-file > resolved-via-import > unique > name-only-ambiguous) with named multipliers (graph.py:52-58) and tests that the label reaches the rendered row and the JSON (tests/unit/test_ref_tiers.py:102-110) are an honest middle ground that is unique in the landscape.
3. **A write-side validation oracle nobody else in this space has.** Baseline/repro/candidate runs in throwaway worktrees with closed stdin, env allowlist, bounded capture, timeout-as-failure, plus AST-equivalence voting (docs/diagrams/validate-vote.md) — navigation MCPs stop at "here are the symbols".

## 6. What is already good (do not re-churn)

- **Determinism is real, not aspirational.** Byte-identical repeated map and communities (measured); all output-reaching set iteration is sorted; the only wall-clock use is duration measurement and cache metadata. Invariant 1 holds.
- **Read-only is enforced where it matters.** Worktree tests assert before/after `git status --porcelain` + HEAD (test_sandbox.py:62-69); a clean checkout was measured after a full validate run; even `patch apply` works in a worktree and emits a diff. Invariant 2 holds.
- **Layering is gated, not documented.** import-linter layers `adapters → application → core → prompts → util` plus CLI/MCP independence: 2/2 KEPT (measured); deptry clean (measured). Invariant 5 holds.
- **Caps are named and tested.** `AUTO_BUDGET_MIN/MAX/DIVISOR` (map_service.py:50-52), `DEFAULT_MAX_TOKENS = 16_000` (envelope.py:55) with hostile-config tests (test_envelope.py:94), fslimits bomb tests (depth bomb, file-count bomb, symlink escape — test_fslimits.py:70-83). Invariant 4 holds.
- **Boundary discipline.** Zero `except Exception`, zero silent catches in `src/`; git subprocesses time out (treewalk.py:34); the RootsFile re-reads on `(mtime_ns, size)` and *refuses loudly* rather than serving a stale allowlist when its file disappears (server.py:352-378). Invariant 6 holds.
- **The receipt + untrusted-content banner on every answer** is the right contract for an agent-facing tool, and the JSON/text parity is pinned by tests (test_cache.py:564-609).

## 7. Appendix

**Commands and key outputs:**

```text
uv run pytest -p no:cacheprovider -q            -> 1562 passed, 49 skipped in 23.33s
uv run lint-imports                             -> Layers ... KEPT; CLI/MCP independence KEPT; 2 kept, 0 broken
uv run deptry src                               -> Success! No dependency issues found
uv run agentless-mcp index --repo .             -> indexed 0, reused 121, errors 21 (pyproject.toml, ci.yml, prompts/*.json,
                                                  tier2 fixtures: "language 'yaml'/'toml'/'json' not warmed"); EXIT=0
time map --no-cache / warm                      -> 1.275s / 0.597s; byte-identical repeat (diff: empty)
time find-symbol build_server --no-cache / warm -> 0.908s / 0.221s
map --focus server.py (auto budget)             -> 8250 chars (~2060 tokens), "59 of 289 symbols shown"
refs build_server                               -> 48 refs; 1 same-file (serve), 45 resolved-via-import
path serve -> personalized_pagerank             -> 4 hops, per-hop kind+tier; unknown target -> "no symbol or file matches"
cycles on a<->b,c->a fixture                    -> exactly "pk/a.py -> pk/b.py -> pk/a.py"; self-repo: "no import cycles"
communities x2                                  -> 24 communities, modularity 0.358; byte-identical
diagram (MCP, max_nodes=8)                      -> 8 nodes + "... 113 more modules" elision node
find-symbol on py/js/rs/go fixture              -> [py:main.py::py_entry] [js:js/bridge.js::jsEntry] [rs:rs/lib.rs::rust_entry] [go:go/main.go::GoEntry]
map/find-symbol on 1,038,917-byte file          -> text: "no matching symbols" / "nothing ... parsed into symbols";
                                                  map JSON: skipped:[{huge.py, "exceeds the per-file cap of 1000000 bytes"}];
                                                  find-symbol JSON: {"total":0,"matches":[]} (no skipped field)
slice escape (symlink to /etc/passwd)           -> "path refused: resolved to /etc/passwd, which is outside the root"
slice with no lines (MCP)                       -> "read_slice requires non-empty lines or explicit whole_file=true"
expand bad id                                   -> "unresolved: ... unreadable: No such file or directory"
validate (baseline green, repro red, 3 cands)   -> 01-fix: apply ok/regression passed/reproduction passed;
                                                  02-wrong: reproduction failed; 03: apply failed "the candidate contains no edits";
                                                  git status clean + HEAD unchanged after; vote -> 01-fix, tier regression+reproduction
patch parse on "*** Begin Patch" text           -> {"edits": [], "errors": []}   (silent no-op)
patch parse on "### path / <<<<<<< SEARCH" text -> 1 edit, 0 errors
lint --diff (unified)                           -> parsed; "not_checked: undeclared_imports (no dependency manifest found)"
grep -rn 'except Exception' src/                -> 0 hits;  'or {}|or []|.get(k,{}|[])' -> 1 benign hit (map_service.py:387)
grep -rn 'subprocess|Popen' tests/unit/test_mcp_server.py -> 0 hits (no transport e2e test)
```

**Distinguishing evidence types:** everything in §2 and §7 is *measured* this session; the 6.002s timeout overrun (D6) and the stdio init hang (D5 context) are *measured in the prior assessment* (docs/functional-assessment.md M1/M3) and re-verified at the code level only; large-repo performance and the "sequential index dominates at 1000+ files" claim are *assumption* (flagged inline).

**Incident disclosure:** one mid-review command chain failed on a `cd` into a cleaned-up `/tmp` scratch dir, and a subsequent line executed in the repo's cwd, committing a scratch `.agentless-mcp.json` to the repository (commit `cf46933`). It was reverted with `git reset --hard ee439cf`; the repo was back at its original HEAD with a clean working tree (verified: `git status --short` empty, `git log -1` = `ee439cf`). No other repository state was touched; all other measurements ran against the read-only checkout or `/tmp` fixtures.

## 8. Observed failure mode: agent tool calls rejected by a client-side validator (D9)

During this review, three consecutive `analyze_structure` (diagram) calls from the reviewing agent failed with an identical validation error, and the fourth — with one parameter dropped — succeeded. This is recorded here as a failure mode in the tool's agent-facing contract, with the correction of the reviewer's initial (wrong) explanation attached, because the first explanation was itself a symptom of the defect.

**What the agent experienced.** Three calls with `focus="server.py"` each returned:

```text
Validation failed for tool "agentless_analyze_structure":
  - focus: must be string

Received arguments:
{
  "operation": "diagram",
  "repo_root": "/home/dallasmarlow/Documents/Development/python/agentless-mcp",
  "focus": [
    "server.py"
  ],
  "max_nodes": 8
}
```

The reviewer's first diagnosis was that the *model* had serialized `focus` as a one-element array, primed by the plural parameter description ("File paths, path suffixes, module names, or symbol names") visible in the client's rendered tool schema. **That diagnosis was wrong.** The session log (`~/.pi/agent/sessions/.../2026-08-22T01-38-18-293Z_...jsonl`, lines 176/179/181) shows the assistant's actual output all three times was the correct JSON string `"focus": "server.py"`. The array appeared only in the error's "Received arguments" echo. So the call was sent correctly, and something between the model's output and the error message transformed a string into a single-element array — while the echoed "received" arguments made the model believe *it* had typed the array, reinforcing the wrong shape on every retry (a self-sealing feedback loop: the more the model retried, the more the echo confirmed its false belief).

**Where the wrapping happens (measured, this session).**

1. The MCP server's wire schema is correct and singular: a live `tools/list` against the running HTTP server (127.0.0.1:8766) returns `"focus": {"type": "string", "description": "Optional file path, path suffix, module name, or symbol name at the centre of a diagram operation."}` — matching `src/agentless_mcp/prompts/parameter_descriptions.json:34` and the installed server copy byte-for-byte. The plural wording the client rendered is produced by the client-side bridge, not by this repository.
2. A direct `tools/call` with `focus` as a **string** succeeds (diagram returned); with `focus` as an **array** the server rejects with a raw pydantic dump: `1 validation error for call[analyze_structure] | focus | Input should be a valid string [type=string_type, input_value=['server.py'], input_type=list]`. So the server itself is sound; the rejection the agent saw was formatted by the client harness.
3. The error format the agent received — `Validation failed for tool "..."` + `Received arguments:` + pretty-printed JSON — is generated client-side by the pi harness at `@earendil-works/pi-ai/dist/utils/validation.js` (`validateToolArguments`). That function deep-clones the arguments, runs TypeBox `Value.Convert` (which coerces values toward the schema), validates, and on failure echoes `JSON.stringify(toolCall.arguments)` — the *post-conversion* object, under the label "Received arguments". That label is the trap: it presents the harness's coerced/transformed view as what the model sent, so a model that is actually sending the right shape is told, with apparent evidence, that it sent the wrong one.

**Why this matters for this tool specifically.** This repository's entire value proposition is that an LLM agent drives it tool-by-tool and reasons over the returned text. A validation error that (a) misattributes the fault to the model, (b) "proves" the misattribution with an echo of transformed arguments, and (c) is byte-identical across retries, converts one recoverable schema mismatch into a multi-retry loop that wastes context and can end with the agent silently dropping a parameter (as happened here: the diagram was finally obtained *without* `focus`, i.e. unfocused, rather than with it) or giving up on the tool. The deterministic, honest-error invariants (invariant 3: weak evidence labeled, never silently promoted) are violated in the meta-channel: the error message is evidence *about the call*, and it was false.

**Ownership split.** Part of this is the client harness (the coercion + the misleading "Received arguments" label), which this repository does not control. But two things are this repository's surface and are fixable here:

1. **Admit the array, or make the rejection actionable.** `repo_map`'s `focus` is a list; `analyze_structure`'s `focus` is a string. The asymmetry is the root cause of the model's confusion (and of the plural rendering). Either accept `str | list[str]` on `analyze_structure.focus` (join or take the first, and say which), or — if the string is intentional — keep it but ensure the *server's* error for a wrong-typed `focus` names the fix: "focus must be a single string, e.g. 'server.py'; for multiple seeds use repo_map". Today the server's raw pydantic dump (`input_value=['server.py'], input_type=list`) is technically correct but gives the agent nothing to act on, and the client's formatter buries it.
2. **Ship a conformance probe for the published schema.** A test that round-trips every tool's `tools/list` schema through a strict JSON-schema validator with one well-typed argument per parameter (and one deliberately mistyped argument per *optional* parameter, asserting the error text names the parameter and the expected type) would have caught both the plural-description drift and the unactionable error. This is the same class of gate as the existing exact-tool-set test (tests/unit/test_mcp_server.py) and fits invariant 7.

**Severity: MEDIUM.** It does not corrupt results and the server-side behavior is correct; it degrades the agent loop (retries, lost parameter, wasted context) and its error message is affirmatively misleading. It is ranked below D1–D3 because those change *answers*; this changes the *cost and reliability of getting answers*.

**Evidence (measured this session):**

```text
session log lines 176/179/181 (assistant output)  -> "focus": "server.py"   (string, all three times)
session log lines 177/180/182 (tool results)      -> "focus": ["server.py"] in the error echo (array)
live tools/list (HTTP 127.0.0.1:8766)             -> focus: {"type": "string", ...}  (singular, matches repo)
direct tools/call focus="server.py"               -> OK, mermaid diagram returned
direct tools/call focus=["server.py"]             -> pydantic: "Input should be a valid string [input_value=['server.py'], input_type=list]"
error formatter                                    -> @earendil-works/pi-ai/dist/utils/validation.js validateToolArguments
                                                     (Value.Convert coercion + 'Received arguments' echo of the converted object)
```
