# Specification: Query-Shaped Loading

## Problem Statement

Every symbol-surface call pays for the whole repository. `scan_repo`
walks the tree, reads and digest-checks every file (the verification
contract), then materializes all three fact kinds for all files into
Python objects, and `build_ref_index` folds them into whole-repo name
maps. Measured on this repository (197 files, 110,308 cached rows,
clean tree, warm, 2026-08-28):

| Component | Cost | Share |
|---|---|---|
| read + sha256 every file | 0.002s | ~1% |
| raw SQLite fetch, all rows | 0.077s | ~30% |
| object materialization + plumbing | ~0.17s | ~69% |

A `find_symbol` call needs a name/parent projection of ~6.8k tag rows
and full rows for at most `limit` matches. A `find_referencing_symbols`
call needs the definitions and sites of one name plus the symbols of
the files that spell it. Both currently construct 102k `Ref` objects
they never read. The goal: construct only what the call needs, while
every call still reads and digest-checks every file, and answers stay
byte-identical.

Out of scope: any cross-call state or memoization (rejected 2026-08-28
-- the contract is per-call verification); `map`, `lint`, `path`,
`cycles`, `communities`, `diagram`, `health` (they consume the whole
scan by design and keep the eager path).

## Research Summary

### Key Findings

- The verification contract is nearly free: 2ms to read and hash the
  full tree. The cost is construction, not trust. (Measured, this
  session; commands recorded below.)
- A secondary index on a `WITHOUT ROWID` table stores the indexed
  column plus the primary-key columns, so `CREATE INDEX ON refs(name)`
  is automatically covering for name -> `(file_id, ordinal)` lookups.
  (Source: sqlite.org/withoutrowid.html, read verbatim.)
- The index trades write and memory overhead for read cost -- the RUM
  conjecture's exact frame. Writes land only in `agentless-mcp index`
  runs, the right side of the trade for a read-mostly cache. (Source:
  Database Internals, "RUM Conjecture", p. 242; DDIA, "Advantages of
  LSM-trees" on write amplification.)
- The lazy source must hide behind the existing seams so callers cannot
  tell eager from lazy -- information hiding is what keeps the module
  deep and the eager path available to the ops that need it. (Source:
  A Philosophy of Software Design, 4.4 "Deep modules", 5.1
  "Information hiding".)
- SQLite's `lower()` is ASCII-only; Python's `str.lower()` is Unicode.
  Any matching done in SQL diverges from `_matches`. Name *equality*
  is safe in SQL (byte equality both sides); substring matching is not.

### Approach Decision

Add a lazy **facts catalog** beside `scan_repo`, not inside it. The
catalog runs the same walk + bounded read + digest check per call,
live-parses only the files whose digest missed (the dirty overlay), and
answers name-shaped questions from SQL restricted to the file ids it
verified this call. `RefIndex` consumers (`resolve_definitions`,
`references_to`) keep their code unchanged by giving the catalog a
`RefIndex`-shaped view whose mappings resolve per name on first access.
All matching semantics stay in Python: SQL narrows by exact name or
returns cheap `(name, parent)` projections; the existing predicates
(`_matches`, `_match_rank`) run unchanged over projected tuples.

### Trade-offs Accepted

- One more full re-index per existing cache (SCHEMA_VERSION 15 -> 16).
  Accepted by the user 2026-08-28.
- Index size on `refs` (~102k entries of short name + two ints here;
  measure with `dbstat` at implementation, expect low single-digit MB
  against a 120MB-class file). Echoes the v13 size discipline: measure
  before claiming.
- Two read paths (eager scan, lazy catalog) that must stay equivalent.
  Paid for with equivalence gates (below), following the existing
  `TestEquivalence` cached-vs-uncached pattern in `test_cache.py`.

## Design

### 1. Verification sweep (unchanged contract, new seam)

Extract the walk/read/digest loop of `scan_repo` into a shared step
that yields, per file: `(path, language, text, file_id | None)` plus
the `SkippedFile` list. `file_id` present means the cache row set for
this exact content is usable; `None` means dirty -> parse live now.
`scan_repo` keeps its signature and behavior for eager callers.

### 2. FactsCatalog

New object in `core/refs.py`, built per call from the sweep:

- `definitions(name)` -> tuple[Definition, ...]
- `sites(name)` -> tuple[Ref, ...]
- `files_referencing(name)` -> int
- `symbols_of(path)` -> tuple[ASTSymbol, ...] (clustered PK read)
- `name_projection()` -> iterable of (name, parent, file_id, ordinal)
  over tags, for `find_symbol`'s substring match and the kind-miss
  `other_kinds` pass
- `imports_all()` -> eager tuple[FileImports, ...] (1.1k rows; the
  resolver needs every file)
- `skipped`, `paths` (walk order)

Rules the catalog owns:

- **Verified-id filter.** Every SQL fetch is filtered to the file ids
  verified this call. A row for a pruned or renamed file, or for a
  file whose content moved, must never leak into an answer.
- **Dirty overlay.** A dirty file's live-parsed facts replace its rows
  entirely for every question.
- **Deterministic order.** Today's answers list definitions and sites
  in walk order, then ordinal. SQL returns arbitrary order; the
  catalog re-sorts every result by `(walk_index(path), ordinal)`.
  This is a byte-identity requirement, not a nicety.
- **Matching stays in Python.** SQL narrows by `name = ?` (byte-equal,
  safe) or hands back projections; `_matches` / `_match_rank` run
  unchanged.

### 3. RefIndex facade

`resolve_definitions` and `references_to` read
`index.definitions.get(name, ())`, `index.sites.get(name, ())`,
`index.files_referencing`. Provide a lazy `RefIndex`-shaped object
whose three mappings are Mapping views over the catalog. The two
functions do not change. `build_ref_index(scan)` stays for eager
callers.

### 4. Cache read side (`core/cache.py`)

- SCHEMA_VERSION 16 with the file's numbered-comment rationale.
- `CREATE INDEX refs_name ON refs(name)` in the schema script
  (covering for name -> (file_id, ordinal) on the WITHOUT ROWID
  table).
- New readers beside the per-file row readers: fetch refs by name
  across many file ids; fetch the tags name/parent projection; fetch
  full tag rows for a set of `(file_id, ordinal)` keys. All go through
  the same `_rows_or_none`-style corruption fallback: persisted bytes
  this build cannot read mean "parse the file", never a failed call.
- No index on `tags` (6.8k rows; a C-speed full scan of the projection
  is sub-ms and substring matching cannot use an index anyway).

### 5. Consumers (phase one: all symbol ops)

- `SymbolService.find_symbol`: sweep -> name projection -> Python
  match -> materialize full rows for matches only.
- `SymbolService.find_referencing_symbols`: sweep -> lazy RefIndex ->
  existing resolution/grouping code; `symbols_of` only for files that
  the matched sites name; `build_resolver` gets `imports_all()`.
  `shared_callers` loads per caller file, bounded by the match set --
  enumerate its exact needs at implementation and fall back to wider
  loads only if a need cannot be name-bounded.
- `GraphService.explain` (via `_resolve`): same catalog path. `path`
  and the whole-repo operations keep the eager scan.
- `expand` / `locate`: already per-file; route their file reads through
  `symbols_of` for consistency, no behavior change.

## Anti-patterns to Avoid

- Matching or lowercasing in SQL (`lower()` is ASCII-only; divergence).
- Caching anything across calls (rejected; the sweep is the contract).
- Making `scan_repo` itself lazy (map/lint/graph consume everything;
  a shallow pass-through that sometimes lies about cost helps nobody).
- Trusting SQL result order (must re-sort to walk order).
- Serving rows for file ids the sweep did not verify this call.

## Task Checklist

- [ ] Extract the verification sweep from `scan_repo`; `scan_repo`
      output byte-identical (existing suites prove it).
- [ ] Schema 16: version comment, `refs(name)` index, migration via
      the existing atomic drop/create; measure db size delta with
      `dbstat` and record it in the CHANGELOG entry.
- [ ] Cache readers: refs-by-name, tags projection, tags-by-keys, with
      the corruption-fallback contract.
- [ ] `FactsCatalog` + lazy `RefIndex` view with the four rules above.
- [ ] Route `find_symbol`, `find_referencing_symbols`,
      `GraphService.explain` through the catalog.
- [ ] Equivalence gates (see Verification).
- [ ] Re-measure: component decomposition + warm medians, clean and
      dirty trees; re-run the codegraph-bench sandbox suite.
- [ ] CHANGELOG entry with measured numbers.

## Files to Modify

- `src/agentless_mcp/core/cache.py` -- schema 16, index, name-keyed
  readers.
- `src/agentless_mcp/core/refs.py` -- sweep extraction, `FactsCatalog`,
  lazy `RefIndex` view.
- `src/agentless_mcp/application/symbol_service.py` -- route find/refs
  through the catalog.
- `src/agentless_mcp/application/graph_service.py` -- route `explain`'s
  `_resolve` through the catalog.
- `tests/unit/test_cache.py` -- extend `TestEquivalence` to the lazy
  path (cached, uncached, dirty).
- `tests/unit/test_refs.py` -- catalog unit tests (order, overlay,
  verified-id filter).
- `CHANGELOG.md`.

## Verification Steps

- [ ] Equivalence sweep: for every name defined or referenced in this
      repository, `definitions`/`sites`/`files_referencing` through
      catalog == through eager `build_ref_index`, on a clean tree and
      with several files dirtied (the PathIndex fix used this A/B
      pattern; zero mismatches is the bar).
- [ ] Rendered-output identity: `find-symbol` and `refs` CLI output
      byte-identical eager vs lazy for a sample of queries, clean and
      dirty.
- [ ] Full unit + characterization suites green; goldens untouched.
- [ ] Latency: re-run the component decomposition and warm medians;
      projection to beat: find <= 0.08s, refs <= 0.15s warm on this
      repository (from 0.25s / 0.35s).
- [ ] Dirty-tree behavior: dirtying one file adds only that file's
      live-parse cost.
- [ ] Cache size: `dbstat` before/after schema 16 on this repository's
      cache; record the delta.
- [ ] Bench: re-run the sandbox suite (same tasks/gold); accuracy
      metrics identical to the 2026-08-28 rematch, latency improved.
