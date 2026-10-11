# Rust Port: Design Records

**Date**: 2026-10-10 | **Source**: the design page "agentless-rs Blueprint", revision 3 (a private claude.ai page) | **Plans**: [2026-10-10_rust-port-00-overview.md](../plans/2026-10-10_rust-port-00-overview.md)

This file copies the records the phase plans cite, so that every id resolves inside the repository. A script generated it from the page's own data, and no row was retyped. The page was written at `1b17fb2` on `feat/erosion-checks` (agentless-mcp 0.9.1). Every file and line in the evidence columns is at that commit: read one with `git show 1b17fb2:src/agentless_mcp/<path>`.

The phase plans supersede the page's crate table, phases, line budget, and decisions table, so this file leaves them out.

## Measurements

Measured on the author's machine (Ryzen AI Max+ 395, Fedora 43) against a clone of mcp-local at `f17af96`, with an edit to the 2,225-line `src/readers/document_readers.py`. The overview's baselines at `1dc4d70` replace the first table's hook figures.

### Where one write goes today

| Step | Time | How measured |
|---|---|---|
| Full erosion hook, no tag cache | 924 ms | hook on stdin payload, 15 runs, median |
| Full erosion hook, warm tag cache | 324 ms | 10 runs, median |
| `clone_delta` alone, no cache / warm | 862 / 251 ms | `lint --path --after --only clone_delta`, median |
| Hook interpreter start (gate off) | 34 ms | 20 runs, median |
| CLI import | ~100 ms | in-process timer; `-X importtime` gives 108 ms |
| Three checks, warm, in process | 141 ms | 20 calls, median; the profile shows 4 parses for 2 texts |
| Git processes per call | 6 | PATH shim log; each takes about 1 ms inside git |

The step rows overlap, so they do not sum to the totals.

### The same work in three languages

| Work | Python 3.13 | Go 1.27 | Rust 1.94 |
|---|---|---|---|
| One edit, new process, parse and measure both sides | 91 ms bare · 200 ms CLI | 23 ms | 13.5 ms |
| Parse 183 files, one thread | 92 ms | 114 ms | 95 ms |
| Find units, count complexity, hash body keys | 195 ms | 145 ms | 23 ms |
| Count source lines (leaf rule) | 80 ms | 86 ms | 16 ms |
| Whole pass, 16 threads | not possible (GIL) | 47 ms | 19 ms |

### The codebase measured by its own tools

| Figure | Value | Source |
|---|---|---|
| Units, and units above complexity 10 | 1,677 / 55 | `agentless-mcp health --repo .` |
| Erosion, clone share | 0.191 / 0.034 | same |
| Source lines: code / docstring / comment / blank | 23,800 / 9,429 / 2,810 / 6,011 | ast and tokenize count over `src/agentless_mcp` |
| Hand-written `as_dict` methods | 79 (843 lines) | ast count over `src/` |
| Grammar libraries: 12 tier-1 / all 22 | 18.5 / 54.6 MB | file sizes in the language-pack cache |
| Test suite | 4,907 passed in 77.3 s | `uv run pytest`; 20.2 s of it is real-clock waits |

## Alternatives

One warm write through the hook on mcp-local. Only the first row is measured. The others are derived from measured parts, and each row states its derivation.

| Option | One warm write | How the figure is derived | Keeps | Costs |
|---|---|---|---|---|
| Today | 324 ms | Measured. | Everything | Nothing new |
| A warm Python process serves the hook | ~175 ms | 141 ms of in-process work plus 34 ms of hook start, both measured. | One language, no port | A daemon lifecycle; Python per-node cost stays |
| The same, plus one parse per side and no duplicate git calls | ~150 ms | Removes about 7 ms of parsing and about 15 ms of git spawn cost from the 175 ms. | One language | Small Python changes |
| Python with a Rust extension for the tree walks | ~190 ms alone · ~90 ms with a daemon | About 90 ms of walks, hashing, and tokenizing run 8 times faster (the prototype ratio); the 100 ms import stays unless a daemon serves the hook. | Most Python code | Two languages, two build systems |
| Free-threaded Python | about 324 ms | Per-call cost is start-up and per-node overhead, not a lack of threads. It could speed a full index build. Support in the tree-sitter binding is not verified. | Everything | A second interpreter build |
| Rust rewrite | target 50 ms or less | The 13.5 ms prototype plus listing, cache, clone lookup, and logging. Not measured; P1 measures it and fails if it misses. | One language, one binary | About 20,000 lines, dual maintenance until P4 |

## Design decisions

Each decision removes a class of finding. The ids name rows in the findings table.

| Decision | Text | Findings |
|---|---|---|
| One parse, one traversal | Each text is parsed once. Parsed keeps the tree-sitter Tree, which queries need, and one traversal writes a flat index beside it. Complexity, line count, and both clone keys read slices of the index instead of walking the tree again. A nested function's key still hashes its own slice, so nested code is hashed once per enclosing key, as today, but it is never walked or parsed again. Today one hook call parses each side up to four times. | A6, C8, C15 |
| Languages are data | One LangSpec per language, plus tags and imports queries, replaces six language lists, five tables in four modules, and most of the per-language handler code. Scope and binding rules stay as code where a query cannot express them. The extractor and the metrics read one table, so they cannot disagree about a name. | A1, A8, C4, C13 |
| One operation table, three doors | Each operation is declared once with a macro: parameters, types, defaults, bounds, descriptions, and an omission rule per door. The macro emits the serde struct, the clap arguments, and the JSON schema. Omission differs by door on purpose: an absent CLI flag defers to project config, and an MCP null, zero, or empty value counts as absent where today's rule says so. A generated test drives every parameter through both doors. | E2, E4, E6, E7 |
| Serde at every boundary | Derive replaces 79 hand-written serializers. Files the project owns (prompts, verdict files) reject unknown fields. Foreign envelopes such as hook payloads validate the fields they need and ignore additions, so an additive upstream change does not break the hook. Project config keeps today's rule: an unknown key is dropped with a warning. Domain checks such as record order and unique ids stay after deserialization. | C2, E3, E5, D2, D6 |
| One check interface | Ten checks implement one Check trait and declare what input they need. The runner builds only that input, applies one error guard and one sort, and emits one Finding type. One cascade matcher serves unit pairing and clone pairing. The three pair checks live below the graph, so the per-write path does not need it. | C5, C6, C9, C14 |
| Ports with fakes | Git, the file system (including writes), processes, the clock, and the store sit behind traits. The application never touches the OS directly. One contract suite runs against each real and fake implementation. A probe has an error channel: no repository is an answer, and a failed probe is an error. | D3, D9, D13, E10 |
| A complete cache key | One table keyed by path, content digest, and an analysis version that hashes the grammar build, the query text, and the metric algorithm. One writer holds the file lock. A corrupt row is rebuilt and reported. Freshness, the index plan, and eviction are pure functions. Today eight tables are read one file at a time, and freshness is decided in two places. | D2, D4, D7, D11 |
| Parity in quarantine | Every Python-specific behavior the goldens need lives in pycompat, one function each, with fixtures from a pinned CPython. When a quirk is no longer wanted, one function and one golden change. | B15, F3 |
| Load once per call | A call loads repository facts once and derives the rest lazily. Today imports resolve twice per graph call, PageRank runs and is discarded, and files are read three times per hook call. | B2, B8, B13, C16, D9 |
| The hook runs in process | agentless hook replaces four Python scripts. There is no second process, no temp file, no JSON round trip, and no capability probe. The hook uses the same config reader as lint, so the two cannot disagree. | E9, F12, F13, F15 |

## Complexity gates

| Gate | What it protects | Mechanism | Runs |
|---|---|---|---|
| complexity_delta, block mode | No function crosses complexity 10, and no function above it grows. | A pinned release runs lint --since $(git merge-base HEAD origin/main) --fail-on warning (a new flag: lint exits 0 on findings today). The Python release gates until P1 ends, then the Rust release. | Per edit (hook) and CI |
| clone_delta, block mode | No new copy of 40 or more tokens. | Same command | Per edit and CI |
| contract_drift on shell crates | No new instance of the silent-default forms in contract_rules.json. Syntactic only. | boundary_globs: crates/{io,git,store,hook,mcp}/** | Per edit and CI |
| No unchecked analysis | A gate that could not run does not pass silently. | --fail-on not_checked: CI fails when any gate reports not_checked | CI |
| Health ratchet | Erosion, units over threshold, and clone share never rise. | CI compares health --json with a committed erosion-baseline.json; only a lower value may update it. Python release until P2. | CI |
| Layer table test | Dependencies point down only. Cargo refuses cycles; this catches the remaining upward edges. | A test reads cargo metadata and checks every edge against the layer table | CI |
| Public API snapshot | Modules stay deep: any growth of a crate's public surface is a reviewed diff. | cargo-public-api snapshot per crate | CI |
| Clippy limits | Function size and nesting limits, and no silent suppressions. | cognitive_complexity 10, too_many_lines, too_many_arguments, excessive_nesting; allow_attributes denied | Pre-commit and CI |
| Unsafe quarantine | Memory safety outside the grammar FFI. | #![forbid(unsafe_code)] in every crate except syntax | Build |
| Mutation score | A small test suite is a strong one. | cargo-mutants on the pure crates, with a floor | Nightly |
| Parity oracle | Byte output does not change by accident. | 57 goldens, scb-check parity, the tokenizer corpus, Loc-Bench ranked_files byte diff | CI and release |
| Dependency audit | Supply chain. | cargo-deny (advisories, licenses, bans, sources) and cargo-machete | CI |

## Review findings

Six review agents each read one block of the code. "Measured" means a command produced the figure, "verified" means the claim was reproduced, and "read" means it comes from reading only. The estimated line savings mix code, prose, and tests. Blocks: A Parsing; B Graph; C Checks; D Storage; E Surfaces; F Tests and hooks; S Self-check.

| Id | Block | Kind | Finding | Evidence | In the Rust design | Est. lines | Basis |
|---|---|---|---|---|---|---|---|
| A1 | A | Duplication | Per-language facts in six language lists and four function-node tables, with about 2,000 lines of per-language handler code. | extractor.py:314-555, 1823-1937, 2082-3966; grammars.py:78-143; symbols.py:135-160; skeleton.py:47-59 | One LangSpec table plus queries. Savings count only after each language's snapshots match. | 1200 | read |
| A2 | A | Waste | Docstrings and comments are about 29% of the block, with provenance and restated code. | extractor.py:103-108, 1476-1491; grammars.py:16-30; slices.py:3-17 | One-line contracts on public items; 1-2 line whys; history in commits. | 1000 | read |
| A3 | A | Design | Grammar download and warm-up machinery with module-level locks and a hidden side effect in a cached getter. | grammars.py:202-637, 207-218, 258-259 | Static grammars reduce this to fn grammar(Language). | 450 | read |
| A4 | A | Duplication | 16 ASTSymbol and 8 ImportStatement construction sites; the type-alias extractor is written twice; signatures built three times. | extractor.py:3656-3679 vs 3858-3885; 2327-2353, 3508-3519, 3738-3749 | Symbol::new with defaults; one signature::compose. | 250 | read |
| A5 | A | Waste | Dead code and unused parameters: _c_function_name has no caller; module_path is ignored by all six import handlers. | extractor.py:3229-3256, 1947-1953, 223-236; symbols.py:84 | Not ported. | 170 | verified |
| A6 | A | Inefficiency | extract_facts makes 4 full walks plus a scope tree (118,112 visits on a 29,528-node file); 13 hand-written traversals. | extractor.py:712-761, 1414-1544; skeleton.py:184-190; complexity.py:181-291 | One traversal per parse. | 120 | read |
| A7 | A | Defect | The Python docstring rule exists in four places and they disagree. A raw docstring r"""Doc line.""" comes back as """Doc line."". | extractor.py:4118-4153; skeleton.py:299-339; normalize.py:473-488; complexity.py:345-349 | One python::docstring function. | 120 | verified |
| A8 | A | Defect | Extractor and metrics name the same code differently: impl<T> Foo<T> gives symbol Foo<T>.bar but unit Foo.bar; a Java record method differs too. | extractor.py:3812-3813, 403-414; complexity.py:404-408, 619-636 | Both read names from the one LangSpec. | 80 | codex |
| A9 | A | Design | TreeSitterExtractor is a namespace class whose handlers append to a shared accumulator instead of returning values. | extractor.py:1820-4202, 38-39 | Pure functions that return Facts. | 0 | read |
| A10 | A | Design | Language is a string; an unsupported one fails four different ways. | extractor.py:44, 2010-2015, 2801-2805 | Language enum; from_path returns Option. | 0 | read |
| B1 | B | Waste | Prose is about 35% of the block, with dated measurements in docstrings. | graph.py:62-83, 470-489; resolve.py:296-314, 998-1015; communities.py:16-37 | One-line contracts; history in commits. | 1900 | read |
| B2 | B | Inefficiency | Imports resolve twice per graph call, and import edges are derived in three places. | graph.py:360-365, 789-806; resolve.py:612-696; graph_service.py:476-499, 850-866 | Build Scopes once; the file graph takes its edges from them. | 80 | read |
| B3 | B | Waste | Community hashing and JSON exist only for tests; passes and converged are computed and never shown. | communities.py:100-110, 140-158, 208-217, 318-340; graph_service.py:366-384 | Delete the hash and passes; show converged when false (recommendation 7). | 60 | verified |
| B4 | B | Duplication | Seven adjacency builders read one edge set; outgoing and incoming regroup the edges on every call. | graph.py:209-236, 472-478, 588-611; communities.py:349-372; mermaid.py:329-332; resolve.py:893-896, 1149-1155 | One forward and reverse CSR per graph. | 50 | read |
| B5 | B | Waste | Checks that cannot fire and no-op branches, such as a weight of 1.0 on both branches. | graph.py:344-349, 410, 608; mermaid.py:286-296; htmlgraph.py:149-164 | Typed indices and validated constructors. | 40 | read |
| B6 | B | Waste | Pass-through wrappers and unreachable branches; two classes named Resolution. | map_service.py:488-501, 806-854; refs.py:270-292; resolve.py:225-231 | Not ported. | 30 | read |
| B7 | B | Duplication | The name-to-file evidence rule is written three times, and spread has three definitions while a docstring calls them equal. | graph.py:395-439, 957; map_service.py:681-684, 1126-1148; symbol_service.py:1016 | One file_evidence function and two named spread functions. | 25 | read |
| B8 | B | Duplication | The ranking pipeline and the order rule are duplicated across services. | map_service.py:309-328; graph_service.py:483-498; graph.py:614-633; mermaid.py:347-352 | One rank_files(&facts, seeds). | 20 | read |
| B9 | B | Inefficiency | Map packing rebuilds every entry on every budget probe and computes each stable id three times. | map_service.py:503-544, 795-803, 1158-1211 | Build entries once, then group by prefix. | 15 | read |
| B10 | B | Design | Stringly typed state: mermaid parses its own n12 ids back to integers to sort. | mermaid.py:490-528; htmlgraph.py:100-117; resolve.py:125-155 | Enums with derived Ord; sort before formatting. | 15 | read |
| B11 | B | Design | 16 functions in the block are above complexity 10, such as build_file_scopes at 19. | resolve.py:612-696; map_service.py:857-881 | A per-statement binder; focus matching as an ordered list. | 12 | read |
| B12 | B | Inefficiency | explain does whole-repository work for one node. | graph_service.py:257-277; resolve.py:513-525 | Use the targeted fan-in that resolved_sites already supports. | 10 | read |
| B13 | B | Inefficiency | communities runs PageRank and an import resolution, then reads only the graph. | graph_service.py:353-384 | Derive values lazily on RepoFacts. | 0 | read |
| B14 | B | Design | Health and the complexity walk live inside the graph service, and services import each other sideways. | graph_service.py:59-68, 502-847; map_service.py:40-48 | A separate health operation; path classes in their own module. | 0 | read |
| B15 | B | Defect | Output depends on the interpreter: sum() is compensated on 3.12+ while the package supports 3.10; some edge order depends on the hash seed. | pyproject.toml:14; graph.py:558, 795-806; communities.py:364-371 | pycompat::fsum_neumaier; sorted iteration. | 0 | codex |
| C1 | C | Waste | 2,241 docstring lines, only 207 on public API; one docstring is false (it says patchlint never calls resolve_elisions). | patches.py:742-747 vs patchlint.py:846-866; normalize.py:29 | One-line contracts; parity notes become named tests. | 2000 | read |
| C2 | C | Waste | Hand-written serialization and a 200-line verdict reader; two functions build the same vote candidate. | erosion.py:77-260; validate_service.py:345-392, 990-1189, 1081-1111 | Serde derive, then the reader's order and uniqueness checks as plain code. | 390 | read |
| C3 | C | Design | A hand-written 380-line Python lexer re-scans text that is already parsed, to check call arity. | patchlint.py:1263-1643 | Typed parameter shapes from parse nodes; keep the conservative refusals. | 300 | read |
| C4 | C | Duplication | Per-language facts in five tables across four modules; the clone function table equals the unit table for all 12 languages. | complexity.py:497-771; clones.py:177-298; normalize.py:265-393 | One LangSpec. | 200 | verified |
| C5 | C | Duplication | The not_checked row is built in about 12 places across three finding types. | lint_service.py:568-596, 829-1061; patchlint.py:728-737; erosion.py:448-461 | One Finding with Finding::not_checked. | 150 | read |
| C6 | C | Duplication | Two copies of the same first-in-first-out multiset matcher, each with O(n) queue.pop(0). | erosion.py:464-548, 805-847 | One cascade matcher with declared stages. | 80 | read |
| C7 | C | Duplication | Three body keys; near_duplicates re-parses every function in the repository to key it. | complexity.py:152-154; normalize.py:405-439; patchlint.py:1221-1260 | One duplicate detector on the measured index. | 80 | read |
| C8 | C | Inefficiency | Five or more walks per file; nested units are walked again by every parent; clone_key is quadratic in depth. | complexity.py:181-291; clones.py:150-165; normalize.py:555-591 | One traversal; keys read slices. | 60 | read |
| C9 | C | Duplication | Parallel value types built by the same expressions (UnitDelta and UnitChange, and two more pairs). | erosion.py:65-90, 181-205, 564-617 | One type per concept. | 50 | read |
| C10 | C | Duplication | The whole-line match rule is written three times. | patches.py:647-731; patchlint.py:869-887; unidiff.py:245 | One find_whole_lines. | 30 | read |
| C11 | C | Design | Stringly typed enums, and erosion imports the 2,129-line patchlint for three names. | erosion.py:24, 47-50; vote.py:166-244; lint_service.py:140-143, 183-207 | Sum types; presentation leaves core. | 30 | read |
| C12 | C | Duplication | _canonical is copied between two services, and one rule is stated in two services. | lint_service.py:1064-1076; patch_service.py:450-465; validate_service.py:1254-1278 | One function. | 25 | read |
| C13 | C | Duplication | Path to language is decided in six places; tier-1 has two definitions. | patchlint.py:787; patch_service.py:446-448; erosion.py:330-333, 721-726; lint_service.py:845-852 | Language::from_path. | 20 | read |
| C14 | C | Inefficiency | lint_patch runs all seven checks, then the service filters by name. | patchlint.py:685-701; lint_service.py:540-544 | Inputs built lazily for the selected checks. | 20 | read |
| C15 | C | Inefficiency | Each side of an edit is parsed up to four times; equivalence keys are computed twice. | erosion.py:359, 435-445, 739-745; contracts.py:68-69; patch_service.py:383-386 | Measure each side once into one value. | 15 | measured |
| C16 | C | Inefficiency | Repository files are read three times per hook call. | refs.py:151-156; lint_service.py:1092-1099, 339-356 | Load once per call. | 0 | read |
| C17 | C | Design | Twelve functions in the block are above complexity 10, such as _write_all at 17. | patch_service.py:543-604; unidiff.py:361, 871 | io::Txn rolls back on Drop; the lexer goes with C3. | 0 | measured |
| D1 | D | Waste | About 38% of the block is prose, including a dated schema changelog; one comment is false. | cache.py:87-184, 192-233, 2106 | One-line contracts; history in commits. | 2000 | read |
| D2 | D | Design | Eight tables and 67 columns, only ever read one file at a time, with hand-written row codecs and unread columns. | cache.py:679-801, 1746-1988, 2086-2213 | One files table with serialized blobs; benchmark blob reads in P1. | 370 | codex |
| D3 | D | Duplication | Two subprocess runners and five copies of the outcome triage. | gitinfo.py:337-470; sandbox.py:374-705; treewalk.py:238-250 | One Runner returning an Outcome enum. | 200 | read |
| D4 | D | Duplication | FileSource has six fetch methods implemented twice; one patch check pays three hashes and up to three parses per text. | cache.py:367-591, 834-843; patchlint.py:827-840 | One FactSource behind one cache-or-parse decorator. | 150 | read |
| D5 | D | Design | Platform dispatch through the strings "posix" and "windows" at eight call sites. | platforms.py:88-94; filelock.py:75, 97; sandbox.py:427-582 | #[cfg] and fs4. | 130 | read |
| D6 | D | Design | Environment and config are read at the point of use in three styles, so a config error appears late. | cache.py:1222-1258; cachedir.py:32-55; projectconfig.py:226-262 | EnvConfig::from_env once at start-up. | 120 | read |
| D7 | D | Defect | Freshness is decided twice and the generation computed three times per call; auto-index opens SQLite on the event loop. | cache.py:944-1010, 1360-1379, 1443-1477; server.py:1216 | A pure freshness function computed once. | 100 | codex |
| D8 | D | Defect | Walk bounds are enforced twice, and two _size_of functions disagree (one keeps an unstatable file, one drops it). | fslimits.py:186-273, 318-323; treewalk.py:149-227 | One bounded walk. | 80 | codex |
| D9 | D | Inefficiency | Six git processes per hook call; the receipt keeps a short SHA that two callers expand again. | gitinfo.py:284-318; treewalk.py:90; history_service.py:161; sandbox.py:261 | One RepoState per call from two processes. | 60 | measured |
| D10 | D | Waste | Grammar warm-up state in the cache. | cache.py:1021-1026, 1653-1670; fileparse.py:58-64 | Gone with static grammars; the analysis version replaces the stamps. | 60 | read |
| D11 | D | Inefficiency | Each receipt runs four COUNT(*) queries, including the largest table, and prints none of them. | cache.py:325-350, 2317-2324 | CacheStatus enum; counts on demand. | 50 | read |
| D12 | D | Duplication | Small duplicates: a SHA pattern twice, rev-parse verify three times, NUL splitting three times. | githistory.py:27, 75-94; gitchanges.py:29, 63-65, 113-117 | One git argv and parse module. | 40 | read |
| D13 | D | Design | Module-level mutable state, including a lock held across git calls of up to 120 s. | cache.py:1288; selfrestart.py:191-205; sandbox.py:193; cachedir.py:55 | Owned state in the server; the deadline is passed explicitly. | 40 | read |
| D14 | D | Design | Rendering utilities sit in the storage-level util package. | util/tokens.py, budget.py, textsafe.py, bounds.py | kernel and render. | 30 | read |
| D15 | D | Design | The index planner reads, decides, and parses in one method. | cache.py:1588-1622 | Walk and read, then a pure plan, then write. | 0 | read |
| E1 | E | Waste | Prose is about 28% of the block, with bug history above one-line bodies. | render.py:41-47; envelope.py:402-415; server.py:1136-1185; symbol_service.py:206-221 | One-line contracts. | 2200 | read |
| E2 | E | Duplication | Each read operation is declared three times and glued twice; the copies have already drifted (read dir takes path, CLI tree does not). | main.py:289-566, 898-1260; server.py:155-424, 645-896, 1532-1877 | The op! table generates both doors; deliberate differences are recorded in it. | 1100 | codex |
| E3 | E | Waste | 36 as_dict methods in render.py; four listings repeat the Sequence protocol. Some transform values or omit fields. | render.py:291-300, 370-389, 598-654, 737-851 | Serde derive with field attributes; one Bounded<T>. | 400 | verified |
| E4 | E | Duplication | The v1 surface duplicates v2 and is kept only for migration. | server.py:1051-1054, 1230-1430, 355-424; cliargs.py:93-99 | Not ported; deprecated in the last Python release (recommendation 2). | 350 | codex |
| E5 | E | Duplication | The typed prompt mirror is hand-written, and about 100 caller-facing strings live outside the JSON. | prompts/__init__.py; loader.py:49-80; main.py help=; server.py:949-1084 | include_str! plus a build.rs that types each message. | 250 | read |
| E6 | E | Duplication | Defaults and bounds live in five places; DEFAULT_FIND_LIMIT alone in six. | symbol_service.py:84, 402; main.py:394; server.py:1351, 1795; capability_service.py:279-323 | Limit<MIN, MAX, DEF> read by every door. | 200 | read |
| E7 | E | Defect | The version is looked up twice with different fallbacks ("0+unknown" and "unknown"); two service bundles are built in two places. | cliargs.py:42-55; capability_service.py:234-239; bootstrap.py:204-217, 285-304 | env!(CARGO_PKG_VERSION); one app::Services. | 120 | verified |
| E8 | E | Duplication | The location text is rendered twice line for line, and renderers live inside services. | server.py:792-804; main.py:1987-2002; symbol_service.py:675-737 | One Render impl per result type. | 60 | read |
| E9 | E | Inefficiency | Start-up imports take 124 ms, argv is parsed twice, and every CLI call builds eight services. | bootstrap.py:184, 204-217, 279; main.py:174-175; server.py:1882 | Parse once; build what the operation needs. | 0 | measured |
| E10 | E | Design | IO inside value objects (RepoContext queries SQLite in __post_init__); the server restarts by sending SIGINT to itself. | repo_context.py:100-154; server.py:1905-1923 | Session::open; restart through a cancel token. | 0 | read |
| F6 | F | Duplication | The services wiring is built in ten test fixtures. | test_cli.py:84-97; test_mcp_server.py:203; test_mcp_surface_v2.py:79-88; test_cache.py:115 | One fake Services builder. | 120 | read |
| F7 | F | Duplication | Repeated setup: one source text in 8 files, context_for in 4, load_hook with a sys.modules hack in 3. | test_map_goldens.py:78-91; test_graph_goldens.py:59-72; test_gate_hooks.py:16-24 | Shared test fixtures. | 200 | read |
| F8 | F | Duplication | Door agreement is pinned in three suites because two copies must be held together. The suites test different properties. | test_cli_bounds.py; test_front_doors.py; test_mcp_surface_v2.py:905-1069 | One generated cross-door test, with independent expected values kept. | 300 | codex |
| F9 | F | Waste | Tests of Python mechanics that Rust does not have: frozen dataclasses, extras, lazy imports, grammar warm-up. Some of these files also pin durable behavior. | test_bootstrap.py; test_token_counter.py; test_grammars.py; test_selfrestart.py | Drop the mechanics; keep each behavioral contract as a Rust test. | 1500 | codex |
| F10 | F | Inefficiency | 20.2 s of the 77.3 s suite is real-clock waiting; one test asserts wall time. | test_sandbox.py; test_mcp_server.py:2284-2317; test_extractor_properties.py:401-427 | Injected clock and deadline. | 0 | measured |
| F11 | F | Design | One test binds a real port and polls with a real clock, and CI runs it on every matrix row. | test_transport_e2e.py:148-212; ci.yml:59-68 | In-process duplex transport; one installed-wheel HTTP run before retirement. | 0 | read |
| F12 | F | Duplication | The erosion hook re-implements config reading, root finding, bounded reads, tier-1 suffixes, and lint JSON parsing. | agentless_erosion_gate.py:348-475, 675-731 | The hook calls the same functions as lint. | 170 | read |
| F13 | F | Defect | The hook opens .agentless-mcp.json directly. Lint refuses a config that resolves outside the repository or is not a regular file, so such a config sets the hook's modes but not lint's. | agentless_erosion_gate.py:394-397; projectconfig.py:197-213 | One config reader. | 0 | verified |
| F14 | F | Duplication | The two gate hooks carry identical append_log and marker_path helpers. | agentless_gate_check.py:92-106; agentless_gate_mark.py:84-98 | One hook crate. | 30 | read |
| F15 | F | Waste | About 250 hook lines exist only for the subprocess protocol: temp file, JSON round trip, capability probe, version skew. | agentless_erosion_gate.py:335-345, 528-672 | In process: none of it exists. | 250 | read |
| S1 | S | Duplication | The branch's own clone_delta finds 6 copies it added: as_dict blocks shared by erosion.py and render.py, and one listing block now held 6 times in render.py. | erosion.py:170-205; render.py:656, 751, 771, 796, 1229-1263 | Serde derive and Bounded<T>. | 0 | measured |
| S2 | S | Defect | complexity_delta in --since mode warns on a test fixture, while health and clone_delta skip test and fixture paths. | tests/characterization/fixtures/complexity_py/branches.py:68 | One scope rule: fixtures skipped everywhere; tests kept only for complexity_delta (recommendation 8). | 0 | measured |

## Defects found

| Defect | Evidence | Basis | Proposal |
|---|---|---|---|
| A raw docstring keeps its quotes: r"""Doc line.""" becomes """Doc line."". | extractor.py:4140 | verified | Fix in Python first, with a predicted golden diff. |
| Class kind is a substring test: Bar(ProtocolError) is a protocol and Foo(Enumerable) is an enum. | extractor.py:3597-3606 | verified | Fix first; compare parsed base names. |
| Symbols and units name a Rust impl method differently (Foo<T>.bar against Foo.bar). | extractor.py:3813; complexity.py:408 | codex | Fix first; one naming rule, with predicted stable-id changes. |
| A Java record method gets parent A as a symbol but A.R.m as a unit. | extractor.py:405; complexity.py:633 | codex | Fix first, with a nested-record fixture. |
| A config that resolves outside the repository, or a FIFO or other non-regular file, is refused by lint but read by the hook. A FIFO can stall the hook. | agentless_erosion_gate.py:394-397; projectconfig.py:197-213 | verified | Fix now in Python; the Rust hook shares the reader. |
| Auto-index opens SQLite on the event loop, and an error at cache.py:1006 can skip connection cleanup. | server.py:1216; cache.py:1006, 1458 | codex | Fix now; offload the work and guarantee cleanup. |
| Two _size_of rules disagree: one keeps an unstatable file, the other drops it. | fslimits.py:323; treewalk.py:184 | codex | Pick one failure rule and its diagnostic. |
| Float output can differ between Python 3.10/3.11 and 3.12+, which the package also supports. | pyproject.toml:14; graph.py:558; communities.py:371 | codex | Goldens are 3.13; pycompat pins that rule; add cancellation-sensitive sums. |
| The version string falls back to "0+unknown" in one place and "unknown" in another. | cliargs.py:42; capability_service.py:239 | verified | Gone in Rust (compile-time version). |
| complexity_delta warns on test fixtures in --since mode. | lint --since output on this branch; lint_service.py:805 | measured | Fix in Python first: skip fixtures in every check (recommendation 8). |
| extract_facts on an unsupported language raises. The empty-result promise is on the symbol and import APIs, not on extract_facts. | extractor.py:1435, 1989 | codex | Define aggregate behavior; Language::from_path returns None. |
| A docstring says patchlint never calls resolve_elisions; it does. | patches.py:743; patchlint.py:861 | codex | Prose not ported. |

## Parity ledger

Behaviors that the goldens, the hook, and other tools depend on. Each one gets one owner in one crate, and a fixture made by a pinned CPython at development time, so no test runs Python.

| Area | Behavior that must match | Home |
|---|---|---|
| Numbers | Python float repr (1e-05, 3.0); round(x, 6) half-even; :.4f, :g, and :.3f; compensated sum() as on 3.12+; negative zero shown as zero; max() keeps the first maximum. | pycompat |
| Text | Lengths in code points (caps 80, 120, 200, 240 with ...); Python whitespace includes \x1c-\x1f and \x85; splitlines() against split("\n"); errors="replace" decoding; one byte-column slice of a str on purpose. | pycompat |
| JSON | json.dumps(indent=2) plus a newline; ensure_ascii with surrogate pairs; insertion order; the compact, sorted, ASCII form inside the HTML export; the fingerprint separators. | pycompat::PyJson |
| Python SLOC | Pinned CPython 3.13 tokenize over splitlines(keepends=True), ignoring COMMENT, NL, NEWLINE, INDENT, DEDENT, ENDMARKER; f-strings per PEP 701; Unicode and indentation rules; rejection and its diagnostic class. | pycompat::python_sloc |
| Diff | Clone pairing moves to the similar crate's Histogram diff, pinned (recommendation 6; no golden changes, measured). file_key hashes a unified diff with lineterm="" over Python splitlines(); keys can differ from Python-built keys when a token holds a line break, so a verdict file is never mixed across the two builds. | checks, patch |
| Ordering | Sorted paths (UTF-8 byte order equals code-point order); stable sorts; rank order (-rank, path) with seeds first; community, cycle, and HTML edge tie rules; first-seen order where Python used dict.fromkeys. | graph, render |
| Ranking | PageRank: damping 0.85, backflow 0.5, strict L1 delta below 1e-6, at most 100 iterations, edge weight count*mult*evidence/(1+ln(1+spread)) left to right. | graph |
| Wire | Tool and parameter names, the 47 parameter and 15 tool descriptions, read-only annotations, _meta alwaysLoad, text-only results; CLI flags, defaults, exit codes 0/1/2, the stdout and stderr split; the per-door omission rules. | app::OPS, prompts |
| Receipt | head, dirty, and cache line formats; the 8-hex generation; the nogit generation hash; envelope constants (16,000 tokens, 1/8 share, 64-token marker). | render, store |
| Capabilities | The grammar capability report and the lint_checks list with each check's input kind, which the hook reads today. | app, render |
| Token counter | The chars/4 default counted in code points; the optional exact counter (tiktoken-rs vendors the tiktoken core; counts must match). | kernel, app |
| Hook files | The JSONL record schema that erosion_gate_report reads; the decision output (additionalContext text, exit 2 with stderr); the search-gate marker file and its location. | hook |
| Reports | The erosion gate report's classes, rates, and labelling CSV. | hook |
| Cache | A clean break: a new file name, so the Python and Rust builds do not delete each other's cache; the same directory scheme and 0700 modes. | store |

## Codex review of revision 1

A read-only Codex review (gpt-6.1-sol) returned FAIL on revision 1: 2 blockers, 34 major, and 11 minor findings. The page lists V28 to V47 one by one and groups V1 to V27, which corrected counts and wording, into one row. The individual texts of V1 to V27 are not on the page.

| Id | Severity | Finding | Change in revision 2 |
|---|---|---|---|
| V28 | blocker | Services belonged to the binary while the doors used it, an upward dependency. | Services moves to app (layer 5). The binary only builds it. |
| V40 | blocker | P1 needed crates that later phases build: checks needed facts, graph, and patch; app needed store. | checks splits into checks (layer 3: the interface and the three pair checks) and patchcheck (layer 4). Each phase lists a dependency-complete crate set; store arrives in P1 with metrics only. |
| V29 | major | Queries cannot run on flat arrays; they need the Tree. | Parsed keeps the Tree. Flat adds named flags and a field index, and its memory cost is recorded in P1. |
| V30 | major | Hashing each nested slice still repeats work. | Decision 1 now says so: one traversal, but each key hashes its own slice, as today. |
| V31 | major | No generator was specified, and omission rules differ by door. | An op! macro emits the struct, clap arguments, and schema, with a per-door omission rule and a generated cross-door test. |
| V32 | major | Path and digest are not a complete cache key. | The key adds an analysis version; one writer under the lock; corrupt rows rebuilt and reported. |
| V33 | major | Tokenizer parity is unproved. | A P1 exit gate: zero differences against a pinned CPython 3.13 over a corpus, plus a fuzz job. |
| V34 | major | The contract gate is syntactic and cannot prove parse-once. | The gate text states its syntactic coverage; types carry the rule; CI fails on not_checked. |
| V35 | major | Rejecting unknown hook fields breaks additive upstream changes; probe had no error channel. | Unknown fields are rejected only in files the project owns; probe returns Result. |
| V36 | major | The line headline left out the data lines. | The headline is now about 26,900 lines, data included. |
| V37 | major | The multiplier and interval were not reconciled. | Every Rust figure is labelled a judgment estimate; P1 gives the first measured check. |
| V38 | major | 25 to 50 ms was not established by the prototype. | It is now a P1 acceptance gate with percentiles, interleaved runs, cold and warm states, and three repository sizes. |
| V39 | major | P0 did not exercise every gate; health gating had no owner before P2. | P0 adds one negative fixture per gate; the Python release keeps the health ratchet until P2. |
| V41 | major | P1 did not test every hook outcome. | P1 adds the hook outcome suite: 12 languages, Edit emulation, advisory and block, malformed payloads, deadline, logging, degraded analysis. |
| V42 | major | The P2 surface diff had no loss rule; the parity ledger missed several contracts. | Zero unexplained losses against a re-run frozen base; the ledger adds the hook log, markers, capabilities, token counter, and reports. |
| V43 | major | No write port; retirement lacked runtime transport tests. | io gains write_atomic and Txn; P3 and P4 add write-failure, rollback, cancellation, roots, HTTP, restart, and installed-wheel tests. |
| V44 | major | Cheaper alternatives were not costed. | New section: Alternatives, on the same workload. |
| V45 | major | No platform targets or size budget. | New section: Platforms, release, and cost, with measured grammar sizes. |
| V46 | major | No cost or dual-maintenance plan. | Lines per phase and a dual-maintenance rule. |
| V47 | major | The release process was not mapped. | The AGENTS.md process is mapped to Cargo and maturin; both console commands stay. |
| V1-V27 | evidence | Counts and wording in several findings were off. | 36 serializers, not 40; 67 cache columns, not about 50; the config-symlink and extract_facts defects reworded; caveats added to A1, C2, C3, F8, and F9. |
