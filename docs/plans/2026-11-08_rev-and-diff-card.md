# Design DES2: `rev` for trace, and the diff-scoped card

## Owner decisions (2026-11-08)

- Both features go into MCP now, through `trace` (`rev` and the `diff:<base>` target), not the CLI only that section 1
  recommends. The schema cost of section 5 (+300 B, +69 cl100k tokens on one root) is accepted. The server plan of
  section 3 (kept state keyed by root and tree) is in scope, and so is the `/pr-review` skill line.
- `rev` ships as R2a (body and history at a revision) and then R2b (every card at a revision).
- The diff card takes the defaults of section 8 as written: 5 rows per section, 12 folders at depth 2, non-test rows
  first, data files counted with their symbols only under a scoped target, a two-revision diff refused in v1;
  `search --rev` and `cat-file --batch` later.

2026-11-08. Branch `feat/trace-tool` at 09847ed, base `main` at 2e8f807 (88 commits). Design only. Raw numbers:
`des2/measurements.txt`; scripts: `des2/scripts/{diffcard,schema_cost}.py`; repository copies: `des2/repos/`.

## 1. Demand, and the recommendation

In 162 Bash calls the REV agent of the `/pr-review` trial ran 3 `git show main:<file> | grep` (pre-images of a
constant and two functions), 1 `git log -L ... main..feat/trace-tool` (2 spans), 13 scope listings (`git diff --stat`,
`--numstat`, `--dirstat`, `--name-status`, `--diff-filter=A|D`) and 5 `git diff -- <file>`. The other 5 trial agents
ran none (their prompts held the file list). Each Loc-Bench snapshot holds 1 commit: no revision to read.

| Feature | Where | Evidence |
|---|---|---|
| `rev` | CLI: `agentless-mcp trace --rev <rev>` | Review sessions only (4 calls of 162 in the one session). In MCP it costs +40 cl100k tokens on every request of every session. `/pr-review` already runs the CLI (`lint --since main`), so one skill line reaches each session that needs it, at 0 schema cost. |
| diff card | CLI: `agentless-mcp trace diff:main` | One call per review, as its entry point; +29 tokens per request in MCP. |
| MCP | not now | Promote a form when the trial (6.4) or the in-situ log shows `git show <rev>:` or `git log -L` outside a skill-driven review. 3.3 gives the server design for that case. |

A call with neither form answers as today; schema, hooks and server do not change. Out of scope: open points 5, 7.

## 2. `rev`: what it applies to

| Target and section | Read at the revision | Cost class |
|---|---|---|
| stable id, `path:line` or `path:start-end` (FIX1b), with `more=body` or `more=history` | one blob, one parse, the symbol by its id qualname | one file |
| symbol card: callers, callees, tests, last change | the revision scan; last change and history run `git log -L <span>:<path> <commit>` | as the card at HEAD |
| file card | declarations, imports, importers, tests, cycles; last change `git log -1 <commit> -- <path>` | one scan |
| folder card, `more=tree`; candidates card; miss | the revision's tree listing, name index and usage files | one scan |

The fast path (first row) serves every pre-image call of section 1 and needs no walk. A name target takes the scan.

## 3. `rev`: design

### 3.1 Reading a revision with no checkout

1. **Resolve.** `Git::verify` (`rev-parse --verify --end-of-options <rev>^{commit}`) gives the commit. A `rev` that
   starts with `-` or holds `:` is refused before git runs.
2. **Walk.** New `Git::tree_walk(root, commit, reads)`: `ls-tree -r -z --long <commit>` from the root (paths relative
   to it, as `ls-files` gives them). It keeps modes 100644 and 100755, passes over links and gitlinks with the walk's
   warnings, takes sizes from the listing, and applies the walk's bounds and `reads` filter.
3. **Changed paths.** `Git::changes(root, commit)` (`diff --raw -z -M --relative <commit> --`, as `lint --since`
   runs it) names each path where the working tree differs from the commit.
4. **Read.** A path the listing does not name has the same bytes on disk, and the scan reads it as today. A named
   path (its old path) is read with `Git::blob`, 4 at a time, as `revision.rs` reads old sides. `scan::scan_one` takes
   the reader as a parameter: the one change to `scan.rs`.
5. **Facts.** Each text goes to `FactSource::facts`. The tag cache matches a row by path and text digest
   (`FactsDb::fresh_row`), so an unchanged file hits its row and a changed file parses in memory.
6. **No write.** The prefix holds `--no-optional-locks` (no index refresh); these calls add `GIT_NO_LAZY_FETCH=1`
   (git 2.44+; here 2.56), so a partial clone never fetches. The fast path reuses `history::Walk::committed_span`
   (blob, parse, find by `id_qualname`), moved to one function for `history`, `expand` and the trace body.

### 3.2 Receipt and rows

The receipt gains one field after `head`: `rev: main 2e8f8074` (the name as given, the short commit); `dirty` and
`cache` keep their working-tree meaning. Each line number and body is the revision's. An id carries no revision: it
is valid at each revision that has its path and qualname. A row `path @line` reads back with the same `rev`, and the
hints of a revision answer say so: `(more=callers, rev=main)`.

### 3.3 Tag cache and kept graph

- **Tag cache:** no key change. It stays a working-tree cache; a revision call never replaces or evicts a HEAD row,
  and the CLI's auto-warm indexes the working tree only. Measured: the HEAD cache serves main's unchanged files.
- **Kept graph, CLI:** none. Each CLI call builds what it reads, as a HEAD call does today.
- **Kept graph, MCP (if promoted):** key a state by (root, tree id), the working tree as (root, none); a revision
  scan shares the root's kept `Arc<FileFacts>` for unchanged paths. `RETAINED.roots` counts states (bound: 2; a druid
  server holds 1.11-1.18 GB with one, TPF2/TPF3b); a dropped state rebuilds on its next call (druid 2.9 s). A
  revision state never updates (fixed tree), so `Stored` and `graph::update` skip it.

### 3.4 Refusals and misses (draft text; the rows go to `crates/prompts/data/*.json`)

| Case | isError | Text |
|---|---|---|
| unknown rev | true | `rev main2 names no commit: <git note>. Give a branch, a tag or a commit id, such as rev=main.` A shallow clone adds `The clone is shallow: the commit can be older than its depth (git fetch --unshallow).` |
| rev with `:` or a leading `-` | true | `rev <rev> is not a revision name: it holds ":" or starts with "-".` |
| path absent at the rev | false | `no file crates/app/src/trace.rs at main (2e8f8074). It is in the working tree: trace it with no rev.` |
| symbol absent at the rev | false | the existing miss text with `at main (2e8f8074)`, then the file card at the revision |
| blob not read (cap, missing object) | false | the file is not parsed: `<path> at main was not read: <git note>` |

### 3.5 Measured cost (des2/measurements.txt; CLI, warm HEAD cache)

| Step | agentless | druid case (2 snapshots, 3,375 files changed) |
|---|---|---|
| resolve + `ls-tree` + `diff --raw -M` | 0.002 + 0.006 + 0.069 s | 0.002 + 0.020 + 0.376 s |
| blobs at main of changed files in a language, 4 at a time | 190 files: 0.046 s | 2,861 files: 0.64 s (0.10 s with `--batch`) |
| symbol card at HEAD (reference) | 0.30-0.32 s, 167 MB | 4.0-4.4 s, 1.15 GB |
| card over main's files with the HEAD cache (emulation) | 0.27 s, 169 MB | 3.5-3.6 s (5.7 s once), 1.11 GB |
| estimate: symbol card at rev=main | about 0.4 s | about 4.6 s |
| estimate: fast path (expand of one file + one blob) | about 0.06 s | about 0.23 s |

The emulation runs `trace --no-auto-warm` on a copy with `main` checked out and a cache built at HEAD: a revision
card's scan, cache hits, live parses and graph build, with disk reads for the blob reads of row 2. Memory as at HEAD.

## 4. The diff-scoped card

### 4.1 Target form and sides

`trace("diff:<rev>")`, and `trace("diff:<rev>:<path>")` for a file or folder. `diff:` resolves before the stable id;
git refuses `:` in a ref name, so the next `:` starts the path. The base is `git merge-base <rev> HEAD` (new
`Git::merge_base`, as a pull request), the new side the working tree with untracked files (`revision::changes`). v1
refuses `rev` with a diff target: `rev does not apply to a diff target. diff:<rev> compares <rev> with the working tree.`

### 4.2 What it lists

```
header  = "diff " rev ": base " sha " (merge base with HEAD), " n " commits, against the working tree" ;
files   = "files: " n " source files (" n " added, " n " removed, " n " changed, " n " renamed) in " n " folders"
          [ "; " n " hidden  (more=tree)" ] NL { "  " folder "/  " n " files  +" n " -" n " ~" n NL } ;
section = ("changed" | "added" | "removed") ": " n " symbols in " n " files, " n " in tests"
          [ "; " n " hidden  (more=declares)" ] NL { "  [" id "] @" span [ " (base @" span ")" ] "  " kind NL } ;
moved   = "moved: " n NL { "  " old_id " @" span " -> [" new_id "] @" span NL } ;
```

Removed rows give the id and span at the base. Changed rows carry both spans: `trace(id, rev=<base sha>, more=body)`
and `trace(id, more=body)` give the two bodies (the composition with `rev`). A moved row pairs a removed and an added
symbol with the same body, name left out (a move or a rename), and leaves the added and removed counts. A scoped file
lists each symbol row in line order (`crates/app/src/trace.rs`: 54 rows, 324 tokens); a scoped folder rolls up.

**Matching** reuses `checks::matching::match_run` (shared by `complexity_delta` and `contract_drift`): Body in another
file or qualname is moved, Name or Collision with other text is changed, New is added, an unpaired old one removed.
It reads `metrics::Unit` (functions) today; D1 makes it generic over a trait (body key, qualname, rank) with no change
in behavior, and a symbol's body key is the digest of its span text, name masked.

### 4.3 Size and caps (des2/scripts/diffcard.py, code suffixes; bytes / cl100k)

| Form | agentless: 326 files; +1,753 -321 ~278, 5 moved | druid case: 3,375 files; +9,586 -3,968 ~9,736, 407 moved |
|---|---|---|
| default: 12 folders at depth 2, 5 rows per section | 2,550 / 895 | 4,994 / 1,387 |
| `more=tree`: each changed file, one line | 18,416 / 6,296 | 392,371 / 103,807 |
| `more=declares`: each symbol row | 98,282 / 33,964 | 1,760,017 / 485,515 |

Row caps bound the default card at any diff size. `more=tree` and `more=declares` reuse `more` values (0 enum bytes)
and stop at `LISTING_LIMIT` (500 rows) and the 16,000-token ceiling with the truncation line; a scoped target reaches
the rest. Data suffixes (json, yaml, toml) add 136 files and 1,701 symbols here (mostly fixture keys as constants):
v1 counts them under `files` and lists their symbols only when scoped. Row order: non-test first, then largest span.

### 4.4 Cost

No graph: changed paths (`diff --raw -M`: 0.07 s / 0.38 s), base symbols (blobs and parse: 0.05 + 0.1 s / 0.64 +
0.6 s), working-tree symbols from the cache. Estimate, CLI: about 0.3 s here, 1.7 s on the druid case.

## 5. Schema cost (des2/scripts/schema_cost.py on `crates/app/fixtures/surface/surface.json`, CHANGELOG method)

| tools/list | bytes (cut entries) | cl100k | `trace` alone |
|---|---|---|---|
| now | 2,340 | 524 | 263 |
| + `rev` parameter and one description sentence | 2,512 (+172) | 564 (+40) | 303 |
| + `diff:<rev>` in the description and the target text | 2,468 (+128) | 553 (+29) | 292 |
| both | 2,640 (+300) | 593 (+69, +13%) | 332 |

CLI only costs 0 bytes: the MCP validator keeps refusing a `rev` property (`additionalProperties: false`), and an
MCP `diff:` target gets the note `diff targets are served by the CLI: agentless-mcp trace diff:main` (isError false).
The CLI text is what the MCP text would be, so a promotion changes only the schema and the descriptions.

## 6. Proof plan

1. **Fixture with a history.** A recipe as in `crates/git/fixtures/history/history.json`: `reuse_rust/base` and
   `reuse_py/base` on `main`, `change/` on a branch, one edited and one untracked file, a shallow clone (fixed
   identity and dates, scratch directory, no ambient git config). Goldens in `goldens/trace/`, text and `--json`:
   rev symbol, file and folder cards, `more=body`, `more=history`, path and symbol absent, each refusal; diff default,
   scoped file and folder, `more=tree`, `more=declares` past the ceiling, a move, an empty diff; `tokens.json` and
   `render.json` rows.
2. **No other change.** Byte-diff of the TPF answer set (1,192 CLI, 894 MCP answers) between the base build and the
   new build: prediction 0 differ. The tools/list golden is unchanged; the `lint --since` goldens are unchanged by D1.
   A test asserts that a rev call leaves `agentless.db` byte-equal.
3. **Differential, free.** On the copies of this branch and the druid case: (a) for each changed and removed symbol
   (here 278 + 321), `trace --rev main <id> --more body` equals `git show main:<path>` cut to the base span; (b) the
   diff counts equal `des2/scripts/diffcard.py`, each difference explained (the script matches by exact qualname).
4. **Trial, on the owner's word.** One `/pr-review` of a real branch, the skill naming `trace diff:main` and
   `trace --rev main <id> --more body`. Count, with `tool_log.tsv` verdicts, the new calls and the drop in `git show
   <rev>:`, `git log -L` and listing `git diff` calls against this trial (3 + 1 + 13 in 162 Bash calls).

## 7. Build phases

One builder per phase, disjoint file sets; each phase runs every gate of `AGENTS.md`, goldens predicted first.
Estimates in agent tokens; past builders ran 1.2-1.8 times their caps.

| Phase | Owns | After | Tests and gates | Est. |
|---|---|---|---|---|
| R1 git | `crates/git/src/{tree,changes,lib}.rs`, `crates/git/src/tree/tests.rs`, `crates/git/public-api.txt` | FIX1a, FIX1b merged | `tree_walk` (modes, bounds, root below the top level, shallow), `merge_base`, `GIT_NO_LAZY_FETCH` on blob and tree reads | 40k |
| D1 matching | `crates/checks/src/{matching,complexity,contract}.rs`, `crates/checks/public-api.txt` | R1 | refactor with no change in behavior: lint goldens and `crates/checks` tests unchanged | 25k |
| R2a fast path | `crates/app/src/{history,expand}.rs`, `crates/app/src/trace.rs`, `crates/app/src/trace/{resolve,symbol}.rs`, `crates/app/public-api.txt` | R1 | one symbol-at-commit function; body and history at rev for an id, `path:line`, a range; the refusals | 50k |
| R2b rev scan | `crates/app/src/scan.rs`, `crates/app/src/symbol_graph/call.rs`, `crates/app/src/search.rs`, `crates/app/src/trace.rs`, `crates/app/src/trace/{file,miss}.rs` | R2a | each card kind at rev; cache rows read, never written | 60k |
| D2 diff card | `crates/app/src/trace/diff.rs`, `crates/app/src/trace/diff/tests.rs`, `crates/app/src/trace.rs`, `crates/app/src/trace/{resolve,section}.rs`, `crates/app/src/revision.rs` | R2b, D1 | target parse, merge base, sections and caps, moves, scoped forms, untracked files | 60k |
| T text | `crates/prompts/**`, `crates/render/src/trace*`, `crates/render/public-api.txt`, `xtask/src/generate.rs`, `goldens/trace/`, `tokens.json`, the fixture recipe | D2 | checklist items 1-3 and 5-7 of the trace plan, section 6; `scripts/gate-selftest.sh` | 50k |
| C CLI, docs | `crates/cli/**`, `crates/mcp/src/` (the CLI-only note), `README.md`, `CHANGELOG.md`, `crates/app/data/agent-guide.md` | T | CLI text equals the goldens; `--help`; the guide test; tools/list golden unchanged | 25k |
| P proof | none (a work folder) | C | 6.2 byte-diff, 6.3 differential, timing on both copies | 30k |

D1 runs beside R2a; R2b and D2 run in sequence (both edit `trace.rs`). Total about 340k, plus the trial. An MCP
promotion is one more phase, about 50k: the surface golden (+300 B, +69 tokens), state keyed by (root, tree) (3.3).

## 8. Open points for the owner

1. CLI only now, MCP on trial evidence; the `/pr-review` skill line (the owner's file; synced to `skills_all`,
   `skills_work`, `pi-mcp-local`).
2. R2a (body and history at rev: every measured call) before R2b (every card at rev), or no R2b.
3. Diff defaults: 5 rows per section, 12 folders at depth 2, non-test first (span order alone puts `GUIDE` first).
4. Data files in the diff card: counted under `files`; symbol rows only under a scoped target.
5. A two-revision diff (`diff:<a>` with `rev=<b>`): refused in v1.
6. `cat-file --batch` (stdin in `io::Spec`): druid blob step 0.64 s to 0.10 s; a typical branch does not need it.
7. `search --rev` (2 of the 3 `git show` calls fed grep): a later design.
