# Rust Port, Phase 2: The Read Surface

**Date**: 2026-10-10 | **Status**: not started | **Overview**: [2026-10-10_rust-port-00-overview.md](2026-10-10_rust-port-00-overview.md) | **Depends on**: P1 released | **Branch**: `feat/rust-port` | **Est. Rust lines**: about 7,300, plus about 2,400 lines of language data and queries

## Goal

Port every read operation: map, symbols (find, overview, expand, explain, locate), refs, read (slice, dir), history, health, cycles, communities, diagram, path, skeleton, and guide. Symbol facts come from one parse through queries and per-language rules. The graph is built once per call. P2 passes when the read goldens are byte-identical and the Loc-Bench retrieval tier ranks every instance the same as 0.10.1.

## Scope

- **In**: `facts` and `graph`, which are new. `store` gains the Rust cache file, its writer, and `index` (moved from P1), with facts, metrics, and auto-index support. `git` gains `log_span`. The read operations and `index` are added to `app`, `render`, and `cli`. Query fields are added to `syntax`. The token counter is added to `render`.
- **Out**: the patch checks, candidates, normalize, validate, and vote (P3); the MCP server (P4). The hook keeps reading the Python `tags.db` until P4 (decision 9).

## Crates at the end of P2

| Crate | Layer | New dependencies or parts in P2 |
|---|---|---|
| `facts` | L2 | syntax, kernel |
| `git` | L3 | none new; adds `log_span` |
| `graph` | L3 | facts, metrics, kernel |
| `store` | L3 | facts; adds the Rust cache file and its writer |
| `app` | L5 | facts, graph |

## Work items, in build order

### 1. `syntax`: extraction data

- **New `LangSpec` fields**, from `src/agentless_mcp/core/extractor.py:314-555` (LANGUAGE_CONFIGS):
  - function, class, and import kinds;
  - the name field and import path kind;
  - identifier kinds and the import binding rule;
  - modifier kinds, and constant kinds and keywords.
- **Stable-id prefix per language**: from `src/agentless_mcp/core/symbols.py:135-160`.
- **Queries per language** in `crates/syntax/languages/<lang>/`: `tags.scm` (definitions, with kind and name captures) and `imports.scm`. Each query is written to reproduce the 0.10.1 output, not copied from upstream tags files.
- **Python builtins**: the list at `extractor.py:1152-1346` becomes a data file.
- **Kept as code where a query cannot express the rule**:
  - Python roles and the scope tree (`extractor.py:712-761`, 864-1050);
  - C and C++ declarators (`extractor.py:3080-3290`);
  - Lua assignments (`extractor.py:2587-2787`);
  - data languages: HCL, SQL, JSON, TOML, YAML (`extractor.py:2792-3075`);
  - require and source calls (`extractor.py:3296-3385`).
  Each sits in its own module, named for the rule.
- **Tests**: every query compiles, and every capture name is one the extractor reads.

### 2. `facts`

- **Types**:
  - `Facts { symbols, imports, refs, errors }`.
  - `Symbol::new(name, kind, node, lang, path)`, with defaults in place of 16 hand-filled constructors (finding A4).
  - `Import` and `Ref { name, line, role, qualifier }`.
- **`extract(&Parsed) -> Facts`**: one pass. Queries give definitions and imports, then the per-language rules give roles and scopes.
- **`StableId`**:
  - the form `prefix:path::qualname#N`, with `FromStr` and `Display`;
  - ordinals from `disambiguate`, by (line, emission index) (`symbols.py:285`);
  - rationale handles `::rationale@L#N` (`extractor.py:1570-1646`).
- **One home per rule** (finding A7): the Python docstring rule, quote stripping, node text, truncation in code points with `...`, and class kinds from parsed base names.
- **Views**:
  - `skeletonize` (`src/agentless_mcp/core/skeleton.py:125-155`, with its sentinel forms and trailing-newline rule);
  - `slice_lines` (`src/agentless_mcp/core/slices.py:44-184`);
  - `resolve_locs` (`src/agentless_mcp/core/locs.py:77-350`).
- **Parity fixtures**: `scripts/facts_parity_fixtures.py` runs once under the frozen 0.10.1 build. It writes the symbols, imports, refs, and roles of every fixture file to `crates/facts/fixtures/`. A Rust test compares each language's output exactly.
- **Tests**:
  - The facts fixtures for all 22 languages.
  - The 10 skeleton goldens.
  - Property tests: `StableId` parse after display is the identity; reordering two top-level functions changes no ordinal of a third.
- **Done when**: every language matches. Line savings from queries count only after this (Codex V18).

### 3. `store`: the Rust cache, facts, and `index`

- **Schema**:
  - `files(path TEXT PRIMARY KEY, digest BLOB, analysis BLOB, metrics BLOB, facts BLOB NULL)`;
  - `meta(schema INTEGER, analysis BLOB, created INTEGER)`.
- `analysis` is a sha256 over the grammar build ids, the `LangSpec` tables, the query texts, and the metric algorithm version. `digest` is a sha256 of the file bytes. Blobs are postcard.
- **Location**: `$XDG_CACHE_HOME/agentless-mcp/<sha256(realpath)[:16]>/agentless.db`, with modes 0700. It is a new file name, so the Python `tags.db` and this file never delete each other.
- **Writers**: one writer under the file lock. A corrupt row is rebuilt and reported.
- **Pure functions**: `plan(prev, observed, analysis) -> Plan` and `evict_plan(...)`. The eviction rules match `cache.py`: 5 GiB default, 0 disables, 24-hour protection, the newest entry and `keep` spared, and coldest first.
- **`FactSource::facts(file)`**: one cache-or-parse decorator over the store, in place of six fetch methods written twice (finding D4). One parse fills both facts and metrics on a miss.
- **`index`**: lists in-scope files, reads and hashes each, builds facts and metrics for the misses in parallel (rayon), and writes in one transaction.
- **`AutoIndexer`**: owned state with one run per root. It is triggered off the calling thread and is used by the MCP server in P4. Its trigger decision is a pure `freshness(meta, live)`, computed once per call (finding D7).
- The Rust CLI read operations use this store. The hook does not use it until P4.
- **Tests**:
  - Table tests for `plan` and `evict_plan`.
  - A state-machine property test: warm equals cold over random edit sequences, for facts and metrics.
  - One parse per miss (a counting fake parser).
  - Corruption and lock-contention tests on a temporary file.
  - Before the read operations rely on the blob layout, compare a blob read with a re-measure on the mcp-local snapshot (Codex V25).

### 4. `graph`

- **`PathIndex`** (`src/agentless_mcp/core/graph.py:240-286`): the only argument type for import resolution, with no scan fallback (finding B2).
- **`NameIndex`**: interned names, definitions, defining files, sites, and two named spread functions (finding B7).
- **`Scopes`**: built once per call. `build_file_scopes` (`src/agentless_mcp/core/resolve.py:612-696`, cc 19) becomes a per-statement binder. The file graph takes its import edges from `Scopes` (finding B2).
- **`Resolver`** (`resolve.py:319-418`): the only owner of `Tier` (`resolve.py:125-157`), an enum with a derived order.
- **`SymbolGraph` and `FileGraph`**: forward and reverse CSR, each built once (finding B4).
  - `SymbolGraph` provides `ambiguous_degree`.
  - `shortest_path` keeps first-found BFS with the edge sort key.
  - `import_cycles` uses petgraph's `tarjan_scc`, then a chain BFS from the smallest member over sorted adjacency.
- **`pagerank`** (`graph.py:492-613`): hand-written. Damping 0.85, backflow 0.5 with no backflow out of a pure source (`graph.py:608`), a strict L1 delta below 1e-6, at most 100 iterations, and the outer loop in sorted node order. Each summation reproduces its own Python site:
  - `totals` and the convergence `delta` use built-in `sum()` (`graph.py:532`, 558): `metrics::compensated_sum`.
  - `dangling` and `incoming` are plain `+=` loops in adjacency order (`graph.py:547-551`): a plain loop in the same order.
  - The update is `(1.0 - damping) * personalization[node] + damping * (incoming[node] + dangling * personalization[node])`, with that grouping (`graph.py:553-556`).
  - Personalization divides each seed weight by a built-in `sum()`, and falls back to a uniform share `1.0 / len(nodes)` (`graph.py:974-985`).
  - Then `rank_order` (`graph.py:614-633`): sort by (-rank, path) with exact float comparison, then the seeds first in that order. Then `flood` (`graph.py:660-720`).
- **`communities`** (`src/agentless_mcp/core/communities.py`): local moving in sorted node order (391). For each node:
  - its degree is subtracted from its community's total before scoring and added to the chosen community after (394, 409);
  - neighbour weights are summed per community in adjacency order (396-399);
  - candidates are scored in ascending community id, and a candidate replaces the running best only when its score is higher by more than 1e-12 (401-407). Port this loop, not the docstring at 47-52, which P0b B10 corrects.
  - `degree` and `two_m` use built-in `sum()` (371-372); the modularity sums at 445-459 keep their order.
  - A community's id is its founding index, and the result sorts by (-size, members). `converged` is reported; the hash and pass count are gone (decision 7).
- **Edge weights**: `count * mult * evidence / (1 + ln(1 + spread))`, evaluated left to right, with accumulation in the Python order: sorted-name references, then imports per statement, then inheritance (`graph.py:314-443`). A contribution of zero or less adds no edge (`graph.py:407`).
- The parity base, the fixture generators, and the stored Loc-Bench base arm all use CPython 3.13.
- **References**: `refs.py` becomes `RefIndex` (`src/agentless_mcp/core/refs.py:115-135`), `resolve_definitions` (295-374), and a sorted span index with binary search for line owners (375-406).
- **Tests**:
  - The 27 graph goldens.
  - Property tests: ranks sum to 1 within 1e-9; a partition covers every node exactly once; two hasher seeds give the same bytes (as `tests/characterization/test_graph_goldens.py:182-222` does for `PYTHONHASHSEED`).

### 5. `app`: read operations

- **`RepoFacts::load(session)`**: scan, index, and scopes once per call. Everything else derives lazily (findings B8, B13, C16). PageRank runs only for an operation that reads ranks.
- **map**:
  - `MapService.build` (`src/agentless_mcp/application/map_service.py:266-436`), with entries built once and grouped by prefix (finding B9);
  - focus seeding (706-881), companions (547-705), and the body map (1028-1087);
  - granularity is an enum.
- **symbols**:
  - find and expand (`src/agentless_mcp/application/symbol_service.py:389-639`, `resolve_symbol_span` 340-388), overview (`src/agentless_mcp/adapters/mcp/server.py:692`), explain (`src/agentless_mcp/application/graph_service.py:235`), and locate (`src/agentless_mcp/application/view_service.py:298`);
  - `explain` uses a targeted fan-in, not whole-repository work (finding B12).
- **refs**: `RefsResult` (`symbol_service.py:270-329`), shared callers (1102-1184, cc 16, split), and site grouping (889-940).
- **read**:
  - slice and dir (`src/agentless_mcp/application/view_service.py:175-296`);
  - `read_slice` (cc 11) is split;
  - the CLI `tree` gains the `path` parameter that MCP `read dir` has (finding E2), as a deliberate difference recorded in `OPS`.
- **history**: `HistoryService` (`src/agentless_mcp/application/history_service.py:130-287`) through `GitRead::log_span`, which this phase adds to `git` (`src/agentless_mcp/core/githistory.py`). The receipt keeps full object ids, so no extra process expands a short SHA (finding D9).
- **orient**:
  - `health` is its own module, out of the graph service (finding B14): orphans, unused exports, hubs, the complexity summary, hotspots, and the walk budget (`src/agentless_mcp/application/graph_service.py:540-836`; `walk_budget` in `src/agentless_mcp/util/deadline.py:93-97`);
  - `cycles`, `communities`, `diagram`, and `path`.
- **guide**: serves `src/agentless_mcp/docs/agent-guide.md` sections through `include_str!` (`src/agentless_mcp/core/guide.py:144-159`). The ground-truth dep-info check refuses an `include_str!` of a file that is not a scanned `.rs` file, so this item needs a decision first: generate the sections into committed Rust with `cargo xtask generate` (P1 work item 2), or add a reviewed list of approved compile-time reads to `xtask/src/ground_truth/dep_info.rs` with a self-test row. No such list exists today (P1 R-48).
- **capabilities** gains the read operations' limits from the same `Limit` constants (finding E6).
- **index**: the CLI command over the `store` writer.
- **Tests**:
  - Every operation runs on fake ports, with insta snapshots of the typed result.
  - The cross-door parameter test is extended to every read parameter.

### 6. `render`: read views

- **Map**: `render_map` (`src/agentless_mcp/application/render.py:1973-2029`, cc 15, split) and the body map (`map_service.py:1087`).
- **Symbols and refs**: cards (`symbol_service.py:788-812`), expansion (640-674), find (703-787), and reference groups (`render.py:2108-2155`, cc 16, split).
- **History**: `history_service.py:288-326`.
- **Health**: the graph sections, the complexity summary, and hotspots.
- **Communities**: the one-line note when not converged.
- **Diagrams**:
  - mermaid (`src/agentless_mcp/core/mermaid.py:191-283`, label allow-list, ids `n{i}`, `_distinct` ordinals);
  - HTML (`src/agentless_mcp/core/htmlgraph.py:64-164`), with the template through `include_str!`, compact sorted ASCII JSON, `<`, `>`, and `&` escaped, and edges sorted lexically by id. The template meets the same dep-info rule as the guide.
- **Numbers**: text uses `{:.4f}`, `:g`, and `.3f`. JSON uses `json::round_half_even(x, 6)`, then `json::float_text`, with negative zero shown as zero.
- **Listings**: one `Bounded<T>` serializer replaces 13 listing classes and 36 `as_dict` methods in `render.py` (finding E3).
- **Token counter**: the default `chars4` counter counts code points, and `--token-counter tiktoken` uses the `cl100k_base` encoding (`src/agentless_mcp/util/tokens.py`; `src/agentless_mcp/bootstrap.py:66`). The Rust port uses `tiktoken-rs` with the same encoding, behind the same flag. The counts must match the Python counts on the fixture corpus, as fixtures generated under 0.10.1 with the `tokens` extra.
- **Tests**: the 12 map goldens and the graph goldens, compared byte for byte (overview rule 6), and insta snapshots over fixed values.

### 7. `cli`: read subcommands

- Every read subcommand is generated from `OPS`. Spellings, defaults, and help match 0.10.1.
- The diagram goes to stdout and its receipt to stderr (`src/agentless_mcp/adapters/cli/main.py:1240-1257`).
- **Tests**: trycmd snapshots of every `--help`.

## Parity harness

- **Goldens**: the 12 map, 27 graph, and 10 skeleton goldens. The Rust tests build the pinned snapshot with a fake `GitRead` (head `0000000f`), as `tests/characterization/test_map_goldens.py:78-86` pins it for Python.
- **Black-box Python tests**: the tests that run the CLI as a subprocess take a driver that runs the Rust binary for read subcommands. Receipt line 2 is normalized. At `1dc4d70` there are 35, all in `tests/unit/test_cli.py`: `TestSubprocess` (8), `TestPatchSubprocess` (21), and `TestValidateAndVoteSubprocess` (6). This gate is narrow: most CLI tests call the adapter in process and do not port. The goldens, the facts fixtures, Loc-Bench, and the surface diff carry the parity proof.
- **Loc-Bench retrieval tier**: run on a frozen copy of the P2 build, per AGENTS.md and the loc-bench-harness. Byte-diff `ranked_files` against the stored 0.10.1 arm. The harness runs `uv run --project <dir> agentless-mcp` (loc-bench-harness `src/loc_bench_harness/agentless.py:79`), so it cannot run a Rust binary. Before P2 starts, add a binary-path option to the harness, in its own repository.
- **Surface diff** (`docs/analysis/benchmark-methodology.md`, "The release regression gate"). The tool exists only in a session scratchpad today. Before P2 starts, commit it to `scripts/`.
  - Write the prediction first.
  - Both builds answer the same queries over the Loc-Bench checkouts.
  - Report tier transitions and a loss count.
  - Read a sample of changed lines in the source.

## Exit gates

- [ ] **Goldens**: the 12 map, 27 graph, and 10 skeleton goldens are byte-identical. The facts fixtures match in all 22 languages.
- [ ] **Loc-Bench**: on a re-run frozen base, `ranked_files` differs in 0 of 150 instances.
- [ ] **Surface diff**: no loss unless each one is a listed decision; every tier transition listed; the sample read.
- [ ] **Black-box tests**: the swapped-driver Python tests pass against the Rust binary.
- [ ] **Recorded, not gated**: `map --no-cache` and `refs --no-cache` latency on a named corpus, against 0.10.1 measured in the same run. The CHANGELOG figures (1,356 and 2,227 ms) do not name their corpus, so they are not the comparison.
- [ ] **Token counts**: the `chars4` and `cl100k_base` counts equal the 0.10.1 fixtures.
- [ ] **Health ratchet**: the Rust build from the `rust-p2` commit takes over the ratchet from Python 0.10.1 (overview rule 2).
- [ ] **Gates**: layers, public API, suppressions, lint tables, clippy, and the erosion gate (the `rust-p1` build) pass.

## Risks

| Risk | Response |
|---|---|
| A query cannot reproduce a handler's output exactly | Keep that rule as code in its own module. The facts fixtures decide, not the line estimate. |
| Float and order differences in ranking or communities | The `json` float functions, `metrics::compensated_sum`, and the stated order rules; the hash-seed property test; byte comparison of the goldens. |
| The Loc-Bench byte diff is not zero | Each differing instance is traced to a rule and fixed. Accepting a difference is your decision, with the instance named. |
| Five functions above complexity 10 move across | Each is split during the port (finding B11). The erosion gate refuses a new one. |
