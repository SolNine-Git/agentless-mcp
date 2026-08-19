# Codebase Audit

Started: 2026-08-19
Scope: src/agentless_mcp/
Auditor: Claude /codebase-review
Strategy: layered (core domain / application services / adapters), blocks = layer x responsibility, leaves-first
Focus: general
Baseline: HEAD b7a97ca, 1140 passed / 4 skipped, 90% total line coverage (.audit/coverage-baseline.json)

## Provenance correction (2026-08-19, post-audit)

The audit did NOT run against a single frozen commit. The tree moved twice while
reviewers were in flight, and the original header overstated the baseline:

- `024f318 fix: diagram import edges read the resolved scopes, not raw statements`
  landed at 14:36:43, one minute after the coverage baseline was captured and as
  wave 1 launched. It touched only `application/graph_service.py` and
  `tests/unit/test_graph_service.py` - i.e. block B19 alone.
- `5ab0eb2 release: 0.2.0` (version bump, pyproject + uv.lock) landed later.

Impact, verified rather than assumed:

- B19's reviewer ran in wave 3, well after 024f318, so its findings describe the
  POST-fix code, which is HEAD's code. B19-H1 (`_imports` slices to `limit` with no
  total or omitted in `ImportRow`, `Explanation.as_dict` or the renderer) and the
  cannot-fail test at `test_graph_service.py:130-136` (`or group.omitted == 0`) were
  both re-confirmed present at 5ab0eb2.
- No other block's source changed between b7a97ca and HEAD, so every other block's
  line references remain valid.
- Coverage re-measured at 5ab0eb2: 1141 passed / 4 skipped, 7100 stmts / 715 missed,
  90% - unchanged from baseline (7101/715). Saved to `.audit/coverage-at-5ab0eb2.json`;
  `coverage-baseline.json` is untouched per the never-overwrite rule.

Conclusion: all findings remain accurate about HEAD 5ab0eb2. Only the single-commit
attribution was wrong. Note also that 024f318 routes the diagram's import edges
through `resolve.build_file_scopes` instead of `graph.resolve_import_target` - which
is independent evidence for B09-H1's fix direction, and leaves B09-H1 itself live,
since `resolve_import_target` still backs `graph.build_graph`'s weighting and
`resolve.py:306,348`.
Executor policy: per-block deep reviews and synthesis run on Opus subagents (user request)
Phase 7 adversarial reviewer: local coder agent via /consult backend (user request, 2026-08-19) - do NOT call gemini

## Status
- Total blocks: 24
- Done: 24 (Phase 4 complete)
- In progress: 0
- Pending: 0
- Phase 4 totals: 5C / 65H / 135M / 152L (357 findings)
- Coverage classes: 3 REFACTOR-READY (B02, B11, B18), 20 COVERAGE-GAP, 1 with TEST-DESERT member (B20 lint_service; B07 has desert sub-regions)

## Block Inventory

Coverage column = line coverage per module from the baseline run; Class is
finalized per block after the qualitative tautology check.

| # | ID | Name | Path(s) | Layer | Deps | LOC | Coverage | Class | Status | C | H | M | L |
|---|----|------|---------|-------|------|-----|----------|-------|--------|---|---|---|---|
| 1 | B01 | Shared utility leaves | util/{errors,platforms,tokens,filelock,fslimits}.py | util | - | 423 | 84% block | GAP | done | 0 | 1 | 6 | 8 |
| 2 | B02 | Prompt text catalog | prompts/{__init__,loader}.py | prompts | - | 220 | 100% | READY | done | 0 | 0 | 4 | 11 |
| 3 | B03 | AST symbol model | core/{symbols,imports}.py | core | - | 299 | 100% (imports.py tautological) | GAP | done | 0 | 2 | 5 | 4 |
| 4 | B04 | Repo discovery and config | core/{gitinfo,projectconfig,treewalk}.py | core | B01 | 726 | 85-96% | GAP | done | 1 | 2 | 8 | 7 |
| 5 | B05 | Output view models and renderers | application/render.py | application | - | 857 | 96% suite / 87% block | GAP | done | 0 | 1 | 3 | 6 |
| 6 | B06 | Grammar loading and language config | core/grammars.py | core | B01 | 314 | 92% | GAP | done | 0 | 2 | 5 | 6 |
| 7 | B07 | Tree-sitter extraction | core/extractor.py | core | B03, B06 | 2090 | 73% | GAP (desert sub-regions) | done | 1 | 6 | 8 | 6 |
| 8 | B08 | SQLite index cache | core/cache.py | core | B01, B02, B03, B04, B06, B07 | 1102 | 92% | GAP | done | 1 | 3 | 7 | 6 |
| 9 | B09 | Reference index and import graph | core/{refs,graph}.py | core | B01, B03, B07, B08 | 548 | 93-95% | GAP | done | 0 | 3 | 5 | 9 |
| 10 | B10 | Symbol edge resolution | core/resolve.py | core | B03, B09 | 811 | 97% suite / 94% block | GAP | done | 0 | 3 | 4 | 7 |
| 11 | B11 | Communities and Mermaid export | core/{communities,mermaid}.py | core | B09 | 765 | 98-100% | READY | done | 0 | 1 | 5 | 7 |
| 12 | B12 | Code view extraction | core/{slices,locs,skeleton}.py | core | B01, B03, B06, B07 | 780 | 95-100% | GAP | done | 0 | 4 | 6 | 6 |
| 13 | B13 | Patch parsing and normalization | core/{patches,normalize}.py | core | B01, B06, B07 | 825 | 95-97% | GAP | done | 0 | 3 | 9 | 6 |
| 14 | B14 | Sandbox execution and voting | core/{sandbox,vote}.py | core | B01, B04, B08 | 731 | 81/100% | GAP (vote READY) | done | 0 | 2 | 6 | 6 |
| 15 | B15 | Patch linting | core/patchlint.py | core | B01, B03, B07, B09, B10, B13 | 1761 | 93% | GAP | done | 0 | 3 | 7 | 7 |
| 16 | B16 | Request context and response envelope | application/{repo_context,envelope}.py | application | B01, B02, B04, B08 | 349 | 96-100% | GAP | done | 1 | 3 | 4 | 5 |
| 17 | B17 | Map and view services | application/{map_service,view_service}.py | application | B01, B02, B04, B05, B07, B08, B09, B12, B16 | 634 | 88-98% | GAP | done | 0 | 2 | 5 | 7 |
| 18 | B18 | Symbol and reference service | application/symbol_service.py | application | B01, B02, B05, B07, B08, B09, B10, B16 | 97% | READY | done | 0 | 3 | 5 | 5 |
| 19 | B19 | Graph, cycles and diagram service | application/graph_service.py | application | B05, B07, B09, B10, B11, B16, B17, B18 | 565 | 99% | GAP | done | 0 | 2 | 4 | 6 |
| 20 | B20 | Patch apply and lint services | application/{patch_service,lint_service}.py | application | B01, B05, B07, B08, B09, B10, B13, B14, B15, B16 | 84/73% | GAP + DESERT (lint_service) | done | 1 | 4 | 6 | 4 |
| 21 | B21 | Validate and vote service | application/validate_service.py | application | B01, B13, B14, B16, B20 | 826 | 91% | GAP | done | 0 | 3 | 7 | 4 |
| 22 | B22 | MCP adapter | adapters/mcp/{server,annotations}.py | adapters | B01, B02, B04, B05, B06, B07, B08, B11, B12, B16, B17, B18, B19 | 715 | 84-100% | GAP | done | 0 | 4 | 6 | 6 |
| 23 | B23 | CLI adapter | adapters/cli/{main,formatting}.py | adapters | B01, B04, B05, B06, B07, B08, B11, B12, B13, B14, B16, B17, B18, B19, B20, B21 | 76-86% | GAP | done | 0 | 5 | 6 | 9 |
| 24 | B24 | Composition root | bootstrap.py | root | B01, B07, B17-B21, B23 (+dynamic B22) | 153 | 71% | GAP | done | 0 | 3 | 4 | 4 |

## Skipped
- Seven empty package __init__.py markers (no code, no edges).
- __main__.py (6 lines, delegates to bootstrap.cli_main).
- tests/ (mapped for coverage assessment, not itself a review target), docs/.

## Segmentation notes (Phase 1, Opus)
- No import cycles, no lazy imports, no layer inversions. Layer contract also
  enforced by import-linter in pyproject.toml (adapters -> application -> core
  -> prompts -> util; CLI/MCP adapter independence).
- God modules: core/extractor.py (2090 LOC, Ca=14, mixes generic walking with
  Python-specific binding-scope analysis), core/patchlint.py (1761 LOC,
  hand-rolled TOML and call-syntax parsers).
- core/cache.py imports prompts (message catalog in storage layer) - needs a
  deliberate ruling in B02/B08 review.
- core/refs.py depends on core/cache.py - domain index importing persistence;
  check for a protocol seam in B09.
- adapters/cli/main.py Ce=27, reaches past application layer into 13 core
  modules; adapters/mcp/server.py milder version (Ce=19, 8 core modules).
- bootstrap loads adapters.mcp.server via importlib (deliberate, test-enforced
  by tests/unit/test_package.py so the CLI never imports fastmcp).
- Known weak regression safety: application/render.py (B05) has no direct test
  file; application/lint_service.py (B20) has no dedicated unit test.

## Cross-block flags surfaced during review
- [B01] adapters catch only AtlasError (cli/main.py:129); OSError/ValueError from util escape as tracebacks - check every block's raise-surface against the typed hierarchy
- [B01] cli/formatting.py:57-68 maps 5 error subclasses onto 2 exit codes with a dead second branch
- [B01] AtlasError is a vestigial name (appears nowhere else in repo; N818 lint exemption pyproject.toml:164-169)
- [B01] walk bound implemented twice: fslimits.py:141-177 and core/treewalk.py:90-115, message text already drifted
- [B01] cache.py:396-404 depends on read_bounded's lossy decode for hash agreement; fslimits H1 fix must be asymmetric (lossy hash, refused write)
- [B01] bounded_walk(include=...) passed nowhere in src/ - sweep for test-only extension points
- [B01] contained_path resolution contract undocumented; lint_service.py:156 / patch_service.py:385 safe only because repo_context.py:71 resolves root
- [B02] cache->prompts edge ruled defensible (prompts stdlib-only, gated by import-linter); real defect is cache.py:996-1006 _rejection prose doubling as control-flow sentinel (return "")
- [B02] CLI-only remediation text on MCP surface: messages.json cache_stale_remediation says "pass --no-cache" (argparse flag) to MCP clients; same at cache.py:108 RECEIPT_BYPASSED
- [B02] "every agent-facing string is data" claim violated at symbol_service.py:214,386, render.py:588,709, cache.py:107-108,142 - no gate enforces it
- [B02] tool_descriptions.json:10 claims cycles op "takes nothing" but server.py:394 reads request.limit and graph_service.py:182 truncates at 20; limit/no_cache/context_lines published without descriptions
- [B02] client-supplied focus seeds echoed unbounded/newline-unsanitized into "# note:" line (map_service.py:183-184)
- [B02] tests/unit/test_mcp_server.py:34-46 EXPECTED_TOOLS duplicates prompts.TOOL_NAMES
- [B02] theme: structural sync machinery strong, nothing catches semantic drift between prose claims and behavior
- [B06] extractor.py:681-685,725-729 catch bare ValueError around get_parser - ABI-broken grammar becomes silent "Unsupported language" + [] extraction
- [B06] extractor.py:634-636 load_language has zero callers in src/ - dead public method
- [B06] tests/conftest.py:32-45 session-autouse real network download on cold cache - suite non-hermetic on first run
- [B06] test_tier2_languages.py:63-69 per-language skip on cold pack cache; tier-2 suite can go green with zero assertions
- [B06] cache.py:491,634,693 use grammars.pack_version() (uncached importlib.metadata) as cache-key input; PackageNotFoundError unhandled
- [B08] ruling: cache->prompts NOT a layering leak (import-linter places prompts below core, written rationale); objection is content only (CLI vocabulary served to MCP clients, B08-L6)
- [B08] concurrency contract unowned: build_index mutates db under write.lock, open_source mutates via _discard without - "one owner per state change" violated for the cache file; flag to CLI index owner (B23)
- [B08] walk_repo called twice per index run and once per read (non-git) - relevant if walk memoization considered (B04)
- [B08] envelope.py:63 evaluates ctx.symbols.receipt 2+ times per response; CachedSource.receipt costs four unmemoized COUNT(*) scans though FileSource protocol advertises a cheap property (B16/B18)
- [B04] refs.py:118 and cache.py:645 call read_bounded(root / walked_path) with no containment check - containment likely belongs at the read (B09/B08)
- [B04] three different git-degradation policies: gitinfo notes-and-answers, treewalk raises, sandbox.py:386 documents a third - no single policy
- [B04] cache.py:407-416 tree_oid as generation sound only because per-file digests also gate reuse
- [B04] envelope.py:136 budget = max_tokens - count(header) with no floor; unbounded repo-controlled receipt empties the body (B16)
- [B04] gitinfo.head_sha/tree_oid/dirty_count have zero production callers
- [B05] omitted-count contract repo-wide: services slice (symbol_service.py:190,247, graph_service.py:449), renderers recompute totals from truncated tuples (render.py:738,776,783) - text output under-reports fan-in
- [B05] envelope.Truncation populated only for map view (main.py:642, server.py:226); all other views leave truncation None (B22/B23)
- [B05] docs/agent-guide.md:502 cites nonexistent docs/diagrams/modules.md; "never-stale committed diagram" story has no working instance
- [B05] no CI: .github/workflows/ absent - import-linter/ruff/mypy enforced only locally (repo-level)
- [B05] "N| " line numbering has four homes with three formats (render.py:767,857; skeleton.py:296,299; slices.py:53; dead render.number_lines)
- [B03] main.py:692 slice --symbol with malformed id raises ValueError traceback out of CLI; other id consumers catch it (B23)
- [B03] slice --symbol only resolves function ids (locs.py:167-186 translates every id to function qualname); class/enum/const ids accepted by expand/explain/refs fail here; TYPE_ALIAS has no location form; ordinal ignored by _resolve_class/_resolve_variable (B12/B23)
- [B03] cache.py tags.qualname written via id_qualname, never selected; ordinal recomputed on read - two homes for one derived fact, stored one unread/untested (B08)
- [B03] extractor.py:650-666 extract_symbols(Path)/extract_imports(Path) callerless; only paths producing absolute-path non-portable stable ids (B07)
- [B03] non-Python handlers ship is_relative=True, relative_level=0 (extractor.py:1309+); graph.py:255/patchlint.py:861 key on those fields (B07/B09/B15)
- [B03] C++ in-class method definitions produce no symbol (B07)
- [B07] grammars.py:38-40 claims tier-1 "characterization coverage" for c/cpp/rust; no fixture exists, no handler line executes (B06)
- [B07] cache.py:657-661 catches only LanguageUnavailable; should adopt patchlint.py:178-188 DEGRADED_ERRORS tuple (incl RecursionError) or one bad file aborts whole-repo index (B08)
- [B07] normalize.py and skeleton.py import four node-type tables from extractor.py - must move to neutral module before any extractor split; main driver of Ca=14 (B12/B13)
- [B07] goldens encode defects: repo_go.map.json has zero Go type symbols and test_stable_ids.py:117-120 asserts the absence - audit goldens against hand-written expectations
- [B07] recursion discipline uneven: resolve.py:733 and patchlint guard explicitly, extractor does not - repo-wide sweep for per-child recursive walks
- [B09] refs->cache ruling: FileSource Protocol properly inverted for types, but Protocol lives inside cache.py and refs.py:111 calls concrete effective_source at runtime - transitive sqlite3 dep real; fix: move Protocol+OnDemandSource to neutral module
- [B09] import-linter treats core as one opaque member - no intra-core direction enforced; recommend intra-core layering (Phase 6)
- [B09] resolve_import_target broken for relative imports (extractor strips dots, startswith(".") never true); resolve.py:306,348 inherit degraded-to-ambiguous resolution; patchlint.py:829 contained
- [B09] set(known_paths) rebuilt per call at graph.py + resolve.py:306,348 - fix all three together
- [B09] three per-file symbol-index implementations with different tie-breaks: refs.symbols_by_qualname (zero callers - delete), resolve.py:673 _owners, patchlint.py:928 _module_level_symbols
- [B11] unvalidated numeric tool params cross MCP/CLI boundary: server.py:616, main.py:357-360 -> graph_service.py:197,241 float(resolution) -> detect_communities, no finiteness/range check (B19/B22/B23)
- [B11] cli main.py json.dumps at six sites with allow_nan=True - non-finite floats reach the wire as bare NaN/Infinity (B23)
- [B11] elision count computed twice with different denominators (mermaid.py:203 vs graph_service.py:259) - text and payload disagree in one response (B19)
- [B11] render.py:459-484 re-implements CommunityPartition.as_dict with different semantics; domain copy test-only (B05)
- [B11] RefGraph.edges iteration order load-bearing for float summation determinism claim (B09)
- [B16] server.py:187 client-advertised roots AUTHORISE (measured: no --root server serves any client dir), pinned by test_mcp_server.py:118, but server.py:96 and repo_context.py:11-14 document the opposite - B22 must rule whether b62b9e3 intended the widening
- [B16] projectconfig.py:201-205 unknown-key warning list uncapped while _stoplist capped - producer half of B16-C1 receipt DoS (B04)
- [B16] cli main.py:1334-1352 _emit drops answer.truncation on --json branch, contradicting _Answer docstring (B23)
- [B16] duplicated replace(ctx, symbols=cache.open_source(...)) at server.py:202-210 and main.py:1310-1318 - natural home is repo_context (B22/B23)
- [B16] path containment guarantee written down only at repo_context.py:11-14 but decided in fslimits/treewalk - confirm in B04/B01
- [B10] patchlint.py:1077 _ARITY_TIERS trusts SAME_FILE/IMPORTED tier for arity signature; B10-H1/H3 could feed wrong signature for bare calls - verify in B15
- [B10] ctx.config.stoplist reaches build_graph and shared-callers but never build_resolver - noise knob has no effect on tiered views (B18/B19)
- [B10] graph_service.explain calls outgoing() and incoming(), each a full rebuild over all edges, to read one bucket each (B19)
- [B10] Relation.INHERITS edges are Python-only (other languages construct bases=()) (B07)
- [B13] one parse-failure invariant, three policies: cli main.py:1218-1221 applies surviving edits and exits 0 on truncated patch; validate_service.py:470-471 correct; lint_service.py:129-141 downgrades to NOT_CHECKED (B20/B21/B23)
- [B13] patch_service.py:342-343 writes new_contents before consulting result.ok - --in-place leaves checkout half-patched (B20)
- [B13] validate_service.py:503 equivalence key gated on bytes-changed; comment-only candidates all hash sha256("") and cluster - can out-vote real fixes (B21)
- [B13] extractor COMMENT_NODE_TYPES/INDENT_BLOCK_NODE_TYPES correctness-load-bearing for equivalence key across normalize/skeleton/patchlint (B07)
- [B13] measured semantic-comment false equivalences: go:build tag swap, shebang python3->2, type comments, noqa/ts-expect-error removal all hash as "same patch" (B21)
- [B14] validate_service.py:614-628 _vote_candidate uses record.get("reproduction") with no presence check while siblings are refused if missing (B21)
- [B14] cli _cmd_vote returns EXIT_OK at TIER_NONE and on UNVERIFIED runs (B23)
- [B14] cli main.py:1398-1406 no ceiling on --timeout, widens disk-exhaustion window (B23)
- [B14] no scratch/cache GC anywhere in src/ for scratch_root(); every skipped _release strands a full checkout under XDG_CACHE_HOME
- [B14] README "no cache or scratch state written inside the repository" falsified by leaked .git/worktrees records (docs)
- [B12] MCP adapter accepts unvalidated list[list[int]] intervals (server.py:557-559) while CLI validates (main.py:1383) - check adapters for the general unvalidated-numeric pattern (B22)
- [B12] b7a97ca fixed silent whole-file fallback in caller (view_service) not primitive (slices._clamp:130) - guard-at-call-site pattern to sweep
- [B12] end_line_number None means "everything after" in slices.py:147 and "one-line span" in locs.py:307; reachable from old cache rows (cache.py:837 decodes NULL) - confirm with B08
- [B12] fixture corpus has zero trailing comments/nested functions/non-ASCII/tabs/CRLF - golden churn warning for new fixtures
- [B15] patchlint._locate re-implements patches._apply_one matching minus elision resolution - elided-edit findings lose line numbers; promote _resolve_elisions to public surface (B13)
- [B15] patchlint._reference_sites ignores Ref.locally_bound - sweep every FileFacts.refs consumer for the same omission (B09/B10)
- [B15] dangling_callers matches names by string equality despite resolver in hand - sweep for resolver-available-but-string-matched lookups (B10)
- [B15] warnings tuple fields need delivery-path sweep: projectconfig's reaches user, patchlint's has no consumer in src/
- [B15] patchlint.py:388 only sys.version_info gate in src/; no CI, requires-python >=3.10 unexercised; hand-rolled TOML scanner diverges from tomllib on this repo's own pyproject.toml
- [B22] CONFIRMS B16 independently: server.py:187 unions client roots into allowlist, --root not a confinement boundary; repo_context docstring asserts guarantee its caller breaks
- [B22] _with_source (server.py:202-210) verbatim duplicate of _context (main.py:1300-1318); capabilities renderings already diverged; import-linter independence forces shared home into application/
- [B22] unvalidated limit sliced raw at symbol_service 190/206/247, negative limit echoed into receipt at 214; mermaid.py:253 validates its equivalent - bound location inconsistent (B18)
- [B22] read_slice empty-intervals contract safe only if callers filter; B22 does not - check CLI path (B23/B17)
- [B22] test_prompts enforces sync but not truthfulness; capabilities description promises "caps in force", handler emits none
- [B22] docs/agent-guide.md:136-140 "exactly one configured root" contradicted by server.py:116-117 default
- [B24] import-linter hole at composition root: nothing forbids bootstrap importing the mcp adapter; optional-extra guarantee rests on source-substring test that misses "from agentless_mcp.adapters.mcp.server import" - add forbidden contract (Phase 6)
- [B24] dynamic imports are a repo-wide static-analysis blind spot (deptry DEP002 exemption for tiktoken); importlib-only blocks need tests since they have no type check
- [B24] no CI confirmed (third block to flag): every "enforced by" claim conditional on installed local hooks
- [B24] tiktoken get_encoding fetches over network with no timeout outside the AtlasError guard - sweep other startup-constructed optional deps
- [B19] map_service owns near-identical scan/index/build_graph/pagerank pipeline that graph_service._ranked hand-mirrors (stoplist included) - one build_file_graph should own it (B17)
- [B19] MCP validates none of max_nodes/limit/resolution for analyze_structure while CLI validates all three; max_nodes=0 escapes as raw ValueError (B22) - third independent sighting of the MCP-validates-less pattern
- [B19] DiagramView.caveat asserts "rank bound left out" for a repo-wide number - focused-diagram honesty fix needs coordinated render change (B05)
- [B19] resolution=inf/NaN reaches json.dumps as bare NaN - global allow_nan=False belongs to JSON-emission owner (confirms B11)
- [B19] Phase-1 sibling-service-import hint does NOT hold up: focus_paths/symbol_card are pure module functions, layers contract intact
- [B17] len(text.split("\n")) line counting duplicated at view_service.py:167/200, refs.py:244, slices.py:93 - all inherit phantom trailing line, "true line count" +1 for newline-terminated files (B09/B12)
- [B17] fourth sighting: MCP adapter validates none of what CLI validates (read_slice ranges, repo_map budget/max_files/granularity, list_dir depth/max_entries) - services trust "the adapter checks" (B22)
- [B17] slices.py:130 "return clipped or [(1, total)]" turns all-intervals-invalid into render-everything - latent for any interval-computing caller (B12)
- [B17] RepoScan.skipped collected by refs, dropped by every text renderer - never-drop guarantee stated once, enforced nowhere (B05/B09)
- [B21] vote.py:232-238 must exclude never-measured candidates; B21-H1 harmful because ladder ranks all-error candidates in applied tier (B14)
- [B21] ApplyStatus lacks not_evaluated member Verdict has - UNVERIFIED run tells vote "patch did not apply" (false) (B14/B20)
- [B21] grammars.get_parser @cache hands one shared mutable Parser to every --jobs thread; GIL masks it now, latent under free-threaded Python (B06)
- [B21] PatchService.apply(in_place=True) is CLI-shaped: _require_clean names --in-place, unconditional sandbox.diff discarded per candidate (B20)
- [B21] .agentless-mcp.json test_cmd decides what gets executed; validate_service docstring denies this path exists (security-relevant, B04/B14)
- [B21] resolve_repo(tree, None) opts out of allowlist authorisation - same call shape as MCP trust boundary (B16/B22)
- [B20] CONFIRMS B01/B13 independently: lossy decode + in-place write corrupts non-UTF-8 bytes (measured cafe-byte -> U+FFFD, exit 0); fslimits errors="replace" is a read decision leaking into the write surface
- [B20] CONFIRMS B13: cli _patch_call drops parsed.errors, half-patch on disk with exit 0 (measured)
- [B20] three load_edits consumers, three error postures (validate refuses, lint reports, CLI proceeds) - needs one cross-block decision
- [B20] load_candidates defined twice (lint_service.py:87, validate_service.py:517) with different collision/encoding behavior; lint docstring falsely claims parity (measured)
- [B20] validate_service.py:485 apply(in_place=True) into worktrees inherits B20-C1 corruption into the judged tree (B21)
- [B20] exit-code inconsistency across write commands (B23)
- [B18] CONFIRMS B05: envelope.Truncation passed by only 1 of 6 limited listings; text/JSON diverge systematically on honesty (as_dict has total/limit, text has neither)
- [B18] refs._parse_one catches LanguageUnavailable per file, _expand_one does not - sweep symbols_for/imports_for/refs_for call sites outside core.refs (B17/B19)
- [B18] CONFIRMS B22: limit is unconstrained bare int on every MCP tool, no Field(ge=1) anywhere in server.py
- [B18] language-neutral claims (_defined_in_tests) verified only for py/java; tier-2 languages have grammar tests but no behaviour tests (B07)

## Suggested commits (user-initiated; this skill never runs git write commands)
- After Phase 2: `audit: initial inventory`
- After each block: `audit: B03 reviewed - 0C / 2H / 5M / 1L`
- After Phase 6: `audit: cross-block synthesis complete`

## Log
- 2026-08-19 Phase 0 complete - fresh audit, scrutinize skill found at user level, tree clean at b7a97ca
- 2026-08-19 Phase 1 complete (Opus executor) - 24 blocks, no cycles, leaves-first order established
- 2026-08-19 Phase 2 complete - .audit/ initialized, coverage baseline saved (never overwrite)
- 2026-08-19 Phase 3+4 wave 1 launched - B01-B08 each on an isolated Opus executor (coverage assessment + 7-pass scrutinize per block)
- 2026-08-19 B01 reviewed - 0C / 1H / 6M / 8L - COVERAGE-GAP (filelock 68%, 2 near-tautological tests); top: fslimits lossy decode + in-place patch write can corrupt non-UTF-8 files
- 2026-08-19 B02 reviewed - 0C / 0H / 4M / 11L - REFACTOR-READY (100%, 4 tautologies noted); top: MCP clients told to "pass --no-cache", a CLI-only flag
- 2026-08-19 B06 reviewed - 0C / 2H / 5M / 6L - COVERAGE-GAP (every broken-grammar branch untested); top: ABI-incompatible grammar -> untyped ValueError swallowed as "Unsupported language", silent empty extraction
- 2026-08-19 B08 reviewed - 1C / 3H / 7M / 6L - COVERAGE-GAP (migrations, rollback, interleavings unpinned); top: B08-C1 stale CachedSource snapshot returns [] symbols/refs with a "fresh" receipt, reproduced against a real db
- 2026-08-19 B04 reviewed - 1C / 2H / 8M / 7L - COVERAGE-GAP (1 tautology, 2 weak asserts); top: B04-C1 walk_repo git path follows tracked symlinks outside repo root, bypassing containment (reproduced)
- 2026-08-19 B05 reviewed - 0C / 1H / 3M / 6L - COVERAGE-GAP (goldens genuine; six empty-states and as_dicts unpinned); top: renderers recompute totals from truncated tuples - text under-reports fan-in (reproduced)
- 2026-08-19 B03 reviewed - 0C / 2H / 5M / 4L - COVERAGE-GAP (imports.py 100% but tautological); top: B03-H1 duplicate_index counts across kinds, locs.py compares within kind - class+def name collision makes the function unresolvable (verified)
- 2026-08-19 B07 reviewed - 1C / 6H / 8M / 6L - COVERAGE-GAP with TEST-DESERT sub-regions (all Rust/C/C++ extraction untested); top: B07-C1 four per-child recursive tree walks, measured RecursionError at 248 chained JS calls; one minified file aborts whole-repo index
- 2026-08-19 wave 1 complete (8/8); wave 2 launched - B09-B16 on isolated Opus executors (Phase-1 hints only, no wave-1 finding bleed)
- 2026-08-19 user directive: Phase 7 adversarial review runs on the local coder agent (/consult backend), not gemini
- 2026-08-19 B09 reviewed - 0C / 3H / 5M / 9L - COVERAGE-GAP (2 non-failing tests in test_graph.py); top: relative-import resolution dead code - every "from . import x" silently loses its 3x-weighted import edge
- 2026-08-19 B11 reviewed - 0C / 1H / 5M / 7L - REFACTOR-READY; top: detect_communities accepts NaN resolution from CLI/MCP - 110 singleton communities with modularity NaN, reproduced end to end
- 2026-08-19 B16 reviewed - 1C / 3H / 4M / 5L - COVERAGE-GAP (boundary guarantees are the uncovered lines); top: B16-C1 receipt header excluded from token budget and uncapped - measured 301,789 tokens from a max_tokens=16000 call via hostile .agentless-mcp.json
- 2026-08-19 B10 reviewed - 0C / 3H / 4M / 7L - COVERAGE-GAP (1 near-tautology); top: same_file tier filters on defined-in-file not module-scope - bare call resolves to Cache.helper over the imported target (reproduced)
- 2026-08-19 B13 reviewed - 0C / 3H / 9M / 6L - COVERAGE-GAP (contract surface uncovered); top: "..." SEARCH/REPLACE block parses clean, applies, injects literal ... line, syntax_delta blesses it (verified end-to-end)
- 2026-08-19 B14 reviewed - 0C / 2H / 6M / 6L - COVERAGE-GAP sandbox / READY vote; top: killed git worktree add leaves locked .git/worktrees record in the analysed repo permanently (verified; prune skips locked)
- 2026-08-19 B12 reviewed - 0C / 4H / 6M / 6L - COVERAGE-GAP (goldens genuine but fixture corpus blind to comments/nesting/encodings); top: trailing comment on first body line makes truncated skeleton indistinguishable from complete function
- 2026-08-19 B15 reviewed - 0C / 3H / 7M / 7L - COVERAGE-GAP (string/comment scanner wholly unpinned); top: B15-H1 malformed pyproject.toml -> known=True with empty package set, every third-party import reported hallucinated (reproduced)
- 2026-08-19 wave 2 complete (16/24); wave 3 launched - B17-B24 on isolated Opus executors
- 2026-08-19 B22 reviewed - 0C / 4H / 6M / 6L - COVERAGE-GAP (3 of 11 tools zero e2e coverage, root-URI parse untested); top: client-advertised roots union into authorization allowlist - independently confirms B16's finding
- 2026-08-19 B24 reviewed - 0C / 3H / 4M / 4L - COVERAGE-GAP (mcp_main executed by zero tests); top: MCP wiring is Any to mypy and untested - field drift ships a console script that dies at startup with every gate green
- 2026-08-19 B19 reviewed - 0C / 2H / 4M / 6L - COVERAGE-GAP (1 cannot-fail test); top: _imports slices to limit with no omitted count - 30 importers rendered as 20 silent rows in the module claiming "says what it left out"
- 2026-08-19 B17 reviewed - 0C / 2H / 5M / 7L - COVERAGE-GAP (view_service error paths unpinned); top: inverted/negative slice ranges render the whole file via MCP (measured), the exact failure the docstring promises it prevents
- 2026-08-19 B21 reviewed - 0C / 3H / 7M / 4L - COVERAGE-GAP (candidate TIMEOUT/ERROR verdicts unpinned); top: three all-error candidates yield tier "applied" and a crowned winner with no warning (measured)
- 2026-08-19 B20 reviewed - 1C / 4H / 6M / 4L - patch_service GAP, lint_service TEST-DESERT (93% incidental, zero owned-behavior tests); top: B20-C1 in-place apply corrupts non-UTF-8 bytes outside the patched region, ok=True (measured)
- 2026-08-19 process note: two wave-3 agents collided on a shared scratchpad probe.py (B24's mypy probe vs B20); no repo files affected
- 2026-08-19 B18 reviewed - 0C / 3H / 5M / 5L - REFACTOR-READY; top: find_referencing_symbols drops 42/52 sites at limit=10 with no omission marker despite tool description promising every match listed
- 2026-08-19 B23 reviewed - 0C / 5H / 6M / 9L - COVERAGE-GAP (151 unpinned stmts, 1 tautology); top: slice --symbol hardcodes function: - class ids answer "no such symbol" exit 0, zero tests on the branch
- 2026-08-19 Phase 4 complete: 24/24 blocks, 358 findings (5C/65H/135M/153L); Phase 5 panel + Phase 6 synthesis launched in parallel
- 2026-08-19 Panel/Architect done - 12 findings (1 CRITICAL: adapter-owned validation asymmetry; panel/architect.md)
- 2026-08-19 Panel/Reliability done - 20 findings (2 CRITICAL: stale-cache silent empty, config-warning token blowout; 9 HIGH incl new: reader deletes db mid-write, list_roots no timeout, unbounded subprocess capture; panel/reliability.md)
- 2026-08-19 Panel/Security done - 18 findings (3 CRITICAL: client roots union into allowlist, git worktree add runs analysed repo's hooks, validate test_cmd falls back to analysed repo config; panel/security.md)
- 2026-08-19 Phase 6 synthesis complete - 12 SYSTEMIC / 12 COUPLING / 9 DRIFT / 13 ASSUMPTION / 5 ARCH patterns (.audit/synthesis.md); upstream-consumer headers filled in all 24 findings files
- 2026-08-19 Phase 7 launched on local coder agent (/consult backend) per user directive, not gemini
- 2026-08-19 Phase 8 consensus written (.audit/consensus.md) - 22 scored clusters, remediation classes, coverage gate applied; noted that the 3-persona vote formula undercounts single-lens findings, so every row also carries REPRODUCED/MEASURED/INFERRED evidence strength
- 2026-08-19 Phase 7 complete (.audit/adversarial.md) - 10/10 claims CONFIRMED, 0 false positives; 4 severity downgrades (root-union, stale-cache, worktree-hooks CRITICAL->HIGH; test_cmd ->MEDIUM), 2 upheld CRITICAL (symlink escape, extractor recursion); B16-C1's 301,789-token figure re-measured at 152,065 (still 9.5x the ceiling); 4 panel misses found (Windows newline rewrite, filelock ENOLCK, filelock O_TRUNC-before-lock, hook runs once per candidate not per run); B21's shared-Parser thread-safety flag CLEARED as a non-finding by stress test
- 2026-08-19 Phase 9 report written (.audit/report.md) - audit complete
