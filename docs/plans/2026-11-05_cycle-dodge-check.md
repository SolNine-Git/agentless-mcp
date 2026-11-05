# Specification: cycle_dodge and mirrored_shapes, two opt-in checks for `agentless lint`

Ticket QCS. Date 2026-11-04. Base: main at 2e87b0a (deployed). Status: design, no code.
Work record: /var/tmp/claude-rust-port/qcs/status.md (W). Pilots: W/scripts, W/data. Web copies: W/web.
"Measured" names a command. "Derived" names a pilot script, which is an approximation of the product.

## 1. The two checks

- **A, `cycle_dodge`.** A function-local import that closes an import cycle when it moves to module
  level. Rule: "Dependencies point one way ... Never break one by moving the import inside a function"
  (~/.claude/CLAUDE.md, Architecture). Ruff PLC0415 flags every local import. A flags only a dodge.
- **B, `mirrored_shapes`.** Two type definitions in different files with mostly the same field names.
  Rules: "Boundary integrity" (hand-mirrored contracts) and "DRY, but not dogmatically" (same CLAUDE.md).

## 2. Code findings (W/clone at 2e87b0a)

- C1. Every resolved import statement is an import edge. `Parts::import_edges`
  (graph/src/symbols.rs:420-437) reads `ImportScope::resolved_edges` (graph/src/scopes.rs:57-65) with no
  position filter. The Python extractor walks the whole tree (facts/src/extract/python_imports.rs:6-14),
  so a function-local import and an `if TYPE_CHECKING:` import are edges.
- C2. `Import` has `line` and no position (facts/src/values.rs:113-131). Nothing tells a module-level
  statement from a function-local one or from a TYPE_CHECKING one.
- C3. `graph cycles` (app/src/cycles.rs:55-71, graph/src/cycles.rs:23-47) lists every strongly connected
  component. Measured (W/probe/dodge, `ag.sh cycles --json`): a cycle a.py -> b.py, closed by a local
  import in b.py, is listed. A cycle through a TYPE_CHECKING import is listed (seaborn copy,
  `seaborn/_core/plot.py -> seaborn/_core/subplots.py`).
- C4. `cycle_delta` (patchcheck/src/cycles.rs:16-55) compares the component file sets before and after
  a patch (lines 45-54). Measured (W/probe/fresh, W/probe/move, `ag.sh lint --diff`): a new local import
  that closes a new cycle gives 1 warning. A patch that moves a module-level import into a function gives
  no row, because the component does not change. `--since` refuses `cycle_delta`.
- C5. `Symbol` has no field list (facts/src/values.rs:31-54). Python `class()` emits methods only
  (facts/src/extract/python_definitions.rs:213-254). A Rust struct is `SymbolKind::Class`
  (facts/src/extract/rust_items.rs:20-24, `adt` at :132).
- C6. `clone_delta` blocks are functions, loops, branches and try statements (agent-guide.md:1519), and
  Type-2 keys ignore names. `near_duplicates` reads function bodies. No check sees a class body.
- C7. The pair table (app/src/registry.rs:95-128) holds each pair check as one row. `callee_overlap`
  (rows 120-127) runs only in `--since` and has its own refusal. `CheckInput` (registry.rs:19-36) holds
  `patch`, `pairs`, `pairs+clones`, `pairs+graph`. Patch checks always run in the candidate forms
  (app/src/lint/candidates/selection.rs:41-55), so a patch check cannot be opt-in today.
- C8. `callee_tree` (app/src/callees.rs:31-46) parses every file under one `Deadline`. `RunInputs`
  (app/src/pair_runs.rs:18-26) and `TreeSources` (app/src/lint.rs:126-130) hold one field for it.
- C9. TYPE_CHECKING has one home: `TYPE_CHECKING` (syntax/src/rules.rs:33), read by
  `SkipRule::TypeChecking` (rules.rs:123-135), private to `syntax`.
- C10. `FACTS_VERSION` is 11 (store/src/analysis.rs:18). The blob pin is store/src/tests/blobs.rs:27-30.
- C11. `health` (measured, `ag.sh health --json`): `over_threshold` 0. No function above cyclomatic
  complexity 10 exists. The nearest in the touched files: `extract.rs::symbols` 9, `extract.rs::imports`
  8, `python_definitions.rs::step` 8. This design adds no branch to any of them.
- C12. The Python-era tree of this repository (021548f^1) has 0 function-local internal imports
  (derived, dodge_pilot.py). History mining here gives no A sample.

## 3. Research findings (prior art)

| # | Finding | Source |
|---|---|---|
| R1 | Pylint R0401's own "problematic code" is a dodge: a function-local import in bad.py, a module-level import back in bad2.py. Pylint reports the cycle at the module-level side. | pylint.readthedocs.io cyclic-import page (W/web/1) |
| R2 | Pylint maintainers keep cycles through function-level imports in R0401 (issues #850, #6983). | WebSearch leads; the issue pages were not read |
| R3 | PLC0415 flags every import outside top level. It names three reasons for a local import: "avoid a circular dependency, defer a costly module load, or avoid loading a dependency". | docs.astral.sh PLC0415 (W/web/2) |
| R4 | import-linter: `acyclic_siblings` contract; `exclude_type_checking_imports` drops TYPE_CHECKING imports from the graph when it is set. | import-linter docs configure.md, acyclic_siblings.md (W/web/8) |
| R5 | tach checks every import, conditional ones included, and skips TYPE_CHECKING imports by default (`ignore_type_checking_imports` true). `forbid_circular_dependencies` is off by default. | tach docs configuration.md, faq.md (W/web/9) |
| R6 | deptry checks declared dependencies (DEP001-DEP005), not cycles. No overlap with A. | deptry.com rules page (W/web/7) |
| R7 | None of R1-R6 separates a dodge from a cycle. All report the cycle, not the hiding import. | R1-R6 |
| R8 | A data clump is "two or three values gathered together". Treatment: Extract Class. | Fowler DataClump bliki (W/web/13); refactoring.guru (W/web/5) |
| R9 | Reek DataClump: 2 or more parameters shared by 3 or more methods; `min_clump_size` 2, `max_copies` 2. Parameters only. | Reek Data-Clump.md (W/web/12) |
| R10 | Data clumps occur across files; detection needs the whole project. Zhang et al. 2008 refine the definition (cited, not read). | arXiv 2403.03903 (W/web/6, W/web/10) |
| R11 | types-checker finds TypeScript interfaces with identical property structures. Exact sets only. | types-checker README (W/web/14) |
| R12 | The docs corpus did not answer: 4 calls failed (3 transport drops, 1 timeout of 1800 s). Stop rule. | W/status.md |

## 4. Pilot measurements (all derived)

**A. W/scripts/dodge_pilot.py**, Python `ast`, approximate resolver, one snapshot of each of 14 Python
projects (W/data/corpus_a.txt, swe-explore-bench). "Dodge": a local import a -> b whose target reaches
a through module-level, non-TYPE_CHECKING imports.

| Measure | Count |
|---|---|
| Files, edges | 10,397 files; 37,101 edges: load 32,702, call 4,178, type_only 185, script 36 |
| Non-test dodge rows | 1,652 (sympy 1,036, astropy 228, pydata 108, django 78) |
| Non-test, return path only through TYPE_CHECKING | 35 |
| Non-test, cycle only through other local imports | 360 |
| Non-test, no cycle | 289 |
| Dodge chain through an `__init__.py` | 1,048 of 1,652; 1-hop chains 565 |
| Local imports with a comment naming a circular import (W/scripts/marked_recall.py) | 83; dodge 63, local-only 10, type-only 2, no edge 8 |

Example (seaborn-3069): `seaborn/axisgrid.py:2157 from .relational import scatterplot  # Avoid circular
import`, chain `relational.py -> axisgrid.py`. The pilot finds it.

**B. W/scripts/shapes_pilot.py.** Rule of section 6.3. Rust fields by a brace scan, Python by `ast`.

| Snapshot | Types with 3+ fields | Pairs | After family limit 5 |
|---|---|---|---|
| this repository 2e87b0a (Rust and scripts/*.py) | 476 | 106 (2 cross-language) | 106 |
| tokio-6752 | 192 | 88 | 28 |
| ruff-15626 | 540 | 100 | 85 |
| nushell-13246 | 236 | 37 | 16 |
| ripgrep-2576 | 66 | 3 | 3 |
| 14 Python snapshots, annotated fields only | 159 | 9 | 9 |
| same, plus class-body assignments and `self.x` in `__init__` | 2,661 | 3,011 | 234 |

A seeded read of 21 kept pairs (seed 20261104, this agent, not blind, not a measurement) shows four
classes: one record in two homes (`app/src/html.rs::HtmlGraph` and `render/src/html.rs::Payload`, 8 of 8);
a generator mirror that the compiler ties (`syntax/src/tables.rs::DeclarationTable` and
`xtask/src/generate/syntax.rs::DeclarationTable`, 16 of 16); siblings (tokio `Write` and `WriteAll`,
django `DatabaseWrapper` per backend, django `Meta`); generic names (`path`, `text`, `language`).

## 5. Reuse verdicts

**A, cycle_dodge.** In part. Reuse: the import graph (`Scopes::build`, `SymbolGraph::imports`), the
statement lines (`resolved_edges`), the BFS shape of `cycle_chain` (graph/src/cycles.rs:140-157), the
TYPE_CHECKING texts (C9), and the `--since` tree-check frame of `callee_overlap`. Not served: no fact
tells a local import (C2); `cycle_delta` misses the move (C4) and is patch-only; `cycles` is not
change-scoped and does not name the hiding statement (C3). Do not extend `cycle_delta`: it is a patch
check, so it cannot be opt-in (C7), and its question (a new component) differs.

**B, mirrored_shapes.** No. No fact holds field names (C5). No check reads a class body (C6). Reuse the
set-overlap design of `callee_overlap` (checks/src/callees.rs:21-27 thresholds, :140-162 inverted index,
:258-290 family): B is its second use. Do not extract a shared overlap module now. The CLAUDE.md DRY
rule waits for the third identical use; the two cores differ in the item pairing (metric units against
type definitions).

## 6. Design

### 6.1 Placement

| | A `cycle_dodge` | B `mirrored_shapes` |
|---|---|---|
| Core (pure) | crates/checks/src/dodges.rs | crates/checks/src/shapes.rs |
| Shell | crates/app/src/dodges.rs | crates/app/src/shapes.rs |
| Graph query | `graph::import_chain` (new, graph/src/cycles.rs) | none |
| Input kind | `pairs+graph` | `pairs+symbols` (new `CheckInput::PairsSymbols`) |
| Forms | `--since` only; own refusal elsewhere | same |
| Severity | `Severity::Advisory` | `Severity::Advisory` |
| Default selection | no; `lint --since REV --only cycle_dodge` | no; `--only mirrored_shapes` |

- Layers (crates/layers.toml): `checks` and `graph` are both layer 3, so the core takes typed rows from
  the shell. `app` (5) calls both. No new crate edge.
- `graph::import_chain(graph, from, to, keeps) -> Option<Vec<Hop>>`: the shortest chain of import edges
  from `from` to `to` over the edges whose (source path, line) `keeps` accepts. It lives beside
  `cycle_chain`.
- `checks::Pair` gains `old_text()` and `new_text()` (checks/src/pair.rs:25-31). The shells extract the
  old-side facts from them.
- Phase 1 does not add either check to xtask/src/erosion.rs `CHECKS` (:10) or to `GatedCheck`
  (io/src/config.rs:102-109).

### 6.2 A: the exact rule

- Statement kinds, from the new fact `Import.runs` (6.4): `load` (runs when the module loads),
  `call` (in a function body), `type_only` (under `if TYPE_CHECKING:` outside a function).
- G(tree): the import graph of a tree. A dodge is a `call` statement S in file a, resolved target
  b != a, such that `import_chain(b, a)` exists over `load` edges only.
- Identity of S: (path, enclosing function qualified name, module, relative level, sorted names).
- Finding set: D(after) minus D(before), by identity. "after" is the working tree. "before" is the
  working tree with each changed file's imports taken from its old text; an added file is absent; a
  deleted file is present.
- `detail.via`: `moved` (before held the same module at `load` in the same file), `new` (other new S),
  `return_path` (S is unchanged; the change made the chain).
- Scope: S in a file that `in_metric_scope` keeps (app/src/scope.rs:57-61). Chain hops: any file.
- Languages: Python. The fact is set only by the Python extractor, so other languages give no `call`
  statement and no row. Rust has no load-time import code, so a Rust `use` cannot dodge. Lua and Ruby
  `require` can, in a later arm.
- Thresholds: none. The claim is binary: the chain exists.
- Order: by path and line. One row per statement.
- Known false-alarm classes: (1) a hop resolved to the wrong file (resolver error); (2) a hop under
  `if __name__ == "__main__":`, which counts as `load` (36 of 37,101 pilot edges); (3) a hop in
  `try: ... except ImportError:` that never runs in practice; (4) a chain through a package
  `__init__.py` re-export: a true cycle, but the fix is to import from the defining module.
- Not reported: a cycle that returns only through other `call` statements (Q2); a return path through
  `type_only` statements (R4, R5); a dodge that the change does not touch.
- Cost: the shared scan (6.6), two `Scopes::build` passes over imports only, one BFS per `call`
  statement of the changed files and per changed edge. No reference resolution.

### 6.3 B: the exact rule

- Type definitions in scope: Python `Class` and `Dataclass` kinds (TypedDict, pydantic models and
  NamedTuple are `Class` with bases); Rust `struct` with named fields. Out: `Enum`, `Protocol`, Rust
  traits, tuple structs, files that `in_metric_scope` drops.
- Fields: the new fact `Symbol.fields` (6.4). Python: names of annotated assignments directly in the
  class body, without `ClassVar[...]`. Rust: the names of `field_declaration` children.
- Normalized name: lowercase, `_` removed, Rust `r#` removed. Thus `other_line` meets `otherLine`.
- Changed types: per pair, old and new types pair by (1) qualified name in one file, (2) qualified name
  in another changed file (a move), (3) an equal field set in one file (a rename). An unpaired new type
  is `new`. A paired type with another field set is `changed`.
- g: every type in scope in the tree after the change, in another file than f, where neither type's
  bases spell the other's own name.
- Rule, s shared normalized names, m and n the smaller and larger set sizes: s >= 3, 5s >= 4m, 2s >= n.
  Reasons: 3 is the size of a clump that is more than a pair (R8, R10); 80% of the smaller set is the
  `callee_overlap` share (the owner accepted it, plan 2026-11-02 sec. 10); the half of the larger set
  keeps a 3-field type out of a 30-field type.
- A `changed` f gives a row only when the change made the mirror: the pair did not meet the rule
  before, or the change added a field that g lacks, or removed a field that g keeps. A change that adds
  one field to both types gives no row.
- Family: skip g when 5 or more other types besides f meet the rule for g (`FAMILY_LIMIT`, as
  callee_overlap). At most 3 rows per f, ranked by s, then larger g, then path and line. A pair of two
  changed types gives one row.
- Cross-language pairs count: a Rust struct and a Python dataclass with one record is the boundary
  case (Q5).
- Known false-alarm classes: siblings (two futures, backend classes, `Meta`); generator mirrors that a
  compiler or test ties; generic names (`path`, `name`, `id`, `limit`, `total`); a wire type and its
  domain type with one converter.

### 6.4 Facts and FACTS_VERSION

- `Import.runs: Runs` with `Load` (default), `Call`, `TypeOnly`; serde `snake_case`, skipped when
  `Load`. python_imports.rs sets it from the ancestors of the statement node: a `function_definition` or
  `lambda` gives `Call`; else an `if_statement` whose condition is a `TYPE_CHECKING` text gives
  `TypeOnly`. `syntax` makes `TYPE_CHECKING` public (C9), so the list keeps one home.
- `Symbol.fields: Vec<String>`, as spelled, in source order, skipped when empty. Set in
  python_definitions.rs `class()` and rust_items.rs `adt()`. Neither change adds a branch to a function
  at complexity 8 or more (C11): each is a new helper that the existing step calls.
- One `FACTS_VERSION` bump, 11 to 12, for both fields. Every tag cache re-indexes once. The blob pin
  (C10) moves to the new digest.

### 6.5 Finding schema and text

Location: the statement (A) or the changed type (B). Both JSON rows add `detail`; the text form does not
print it (render/src/lint.rs needs no change).

| Row (messages.json) | Text |
|---|---|
| `cycle_dodge_found` | "The import of `{module}` in `{function}` runs only when the function runs. At module level it closes this import cycle: {chain}. Break the cycle: move the shared code to a module that both files import, or invert the dependency." |
| `cycle_dodge_found_evidence` | "{path}:{line} imports {target}; {target} imports {path} back through {hops}" |
| `cycle_dodge_old_not_parsed` / `_new_not_parsed` | "not checked: cycle_dodge did not compare {path}, because its text before (after) the change does not parse. {reason}" |
| `mirrored_shapes_found` | "`{name}` has {count} of its {total} field names in common with `{target}` in another file, which has {other_total}. If the two hold one record, give the record one home. If a boundary needs two types, convert in one function and test that the fields match." |
| `mirrored_shapes_found_evidence` | "`{target}` at {path}:{line}; shared: {listed}; only here: {here}; only there: {there}" |
| `mirrored_shapes_old_not_parsed` / `_new_not_parsed` | as for cycle_dodge |

- Shared rows (third use of one text, CLAUDE.md DRY): unit U0 turns `lint_callee_overlap_since_only`
  and `callee_overlap_stopped` into `lint_tree_check_since_only` and `tree_check_stopped` with `{check}`
  and `{reads}` values. The callee_overlap text stays byte-identical. `callee_overlap_stopped_evidence`
  ("time budget spent") serves all three.
- `detail` of A, `Detail::Dodge`: `module`, `target`, `function`, `via`, `chain` (each hop `{path, line,
  target}`).
- `detail` of B, `Detail::Mirror` (boxed, as `Overlap`): `unit`, `via` (`new`, `changed`), `kind`,
  `other`, `other_path`, `other_line`, `other_kind`, `shared` (each `{here, there}` as spelled), `only_here`,
  `only_there`, `family`, `before` (whether the pair met the rule before the change).
- Count: 8 new message rows, 2 renamed. messages 252 -> 260 (xtask/src/generate.rs:227); catalog 265
  -> 273 (crates/prompts/src/tests.rs:105). Each row gets a crates/prompts/fixtures/render.json row with
  sample values.
- Agent guide (crates/app/data/agent-guide.md): two rows in the check table (:1509-1521), a paragraph
  each after "`callee_overlap` compares what the functions call" (:1598), the `detail` fields (:1456),
  the `--since` list (:1413), and the measured cost.

### 6.6 The shared tree scan (unit U0)

Each tree check adds one field to `RunInputs`, `TreeSources` and `lint_with` (C8): a parallel ladder.
With a third check, U0 refactors first, with no change in behavior: `TreeSources` holds one lazy scan
(`RepoFacts` under one `Deadline`, built on first use), and `callee_tree` builds its graph from that
scan. A run with all three checks then parses each file once. Proof: every suite and golden is
unchanged.

## 7. Tests, fixtures and goldens

- Unit tests: checks/src/dodges/tests.rs (identity diff, `via`, order, stop row); checks/src/shapes/
  tests.rs (thresholds at the edges 3/2, 4/5 and 1/2, family, row cap, the drift rule, once per pair,
  normalization); graph/src/tests (import_chain with a filter); facts tests (`runs` per context, `fields`
  per kind); registry tests (rows, refusals, selection).
- Fixture repositories, each with base/, change/ and planted.json, as callee_overlap_py:
  - tests/characterization/fixtures/cycle_dodge_py. Positives: a module-level import moved into a
    function (2 files); a new local import that closes a 3-file chain; a chain through an `__init__.py`;
    a relative import in a method; a new module-level import that makes an old local import a dodge.
    Negatives: no return path; a return path only through TYPE_CHECKING; a cycle only through local
    imports; a local import in a test file; an old dodge the change does not touch; a third-party target.
  - tests/characterization/fixtures/mirrored_shapes_py and mirrored_shapes_rust. Positives: a new
    dataclass that mirrors a model in another file; a Rust struct that mirrors one in another crate; a
    changed type that drifts; a Rust and Python pair (in the py fixture). Negatives: a subclass that
    names its base; a same-file pair; 2 shared fields; a family of 6; an enum; a type in a test path; a
    change that adds one field to both sides.
- New goldens: tests/characterization/goldens/lint/{cycle_dodge_py,mirrored_shapes_py,
  mirrored_shapes_rust}.lint.json. The test asserts that every row is a planted positive and that no row
  names a planted negative.
- Predicted diffs of existing goldens (predict each line before you keep it):
  - crates/facts/fixtures/extract_python.json: `runs` on each local and TYPE_CHECKING import (the file
    holds 9 TYPE_CHECKING mentions); `fields` on each class with annotated fields (23 dataclass symbols).
  - crates/facts/fixtures/extract_rust.json: `fields` on each struct with named fields.
  - store/src/tests/blobs.rs: PINNED (12, new digest).
  - Every list of all checks gains the two names at its end: crates/app/fixtures/{capabilities,registry,
    write/lint}.json, crates/cli/fixtures/{parser,scenarios,read/scenarios}.json,
    crates/mcp/fixtures/calls.json, the guide fixture. capabilities gains the line
    `pairs+symbols: mirrored_shapes`.
  - crates/hook/src/modes/tests.rs asserts that callee_overlap is the one pair check the hook leaves
    out. It then names three. The public-api lists of checks, graph and facts gain the new items
    (`cargo xtask public-api`).
  - No read golden (map, find, refs, skeleton, graph) changes: `runs` and `fields` are not in their
    views. If one changes, run the surface diff (AGENTS.md).

## 8. Measurement before any gate (the CO1B method)

The method of docs/analysis/2026-11-04_callee-overlap-measurement.md: blind packets, two agent readers in
two rounds (the first 20, then the rest), each round without the other's labels; Cohen's kappa; the owner
decides each disagreement; the owner reads 20 seeded findings, and if agent-owner kappa is below 0.6 the
owner labels every finding. False-positive (FP) rate with its Wilson 95% interval. Seeds 20261104.
Write every prediction before its run. Runs use detached copies with an absolute XDG_CACHE_HOME (AGENTS.md).

**A, cycle_dodge.**
- M1: the planted fixture; recall per positive kind; 0 rows on negatives.
- M2 recall: the 83 comment-marked local imports of the 14 snapshots (section 4). Recall = rows / marked.
- M4 sample: each snapshot copied over an empty root commit (as CO1B M4); `lint --since <root> --only
  cycle_dodge --json`, so every statement is new. Sample 60 rows, at most 6 per snapshot, water-filled.
- M3: changes. This repository has no sample (C12). Phase 1 uses M4 only and states that limit.
- Packet: the statement with 3 lines around it and its function signature; each hop as file, line and
  statement text; read access to the copy. Hidden: `via`, the rule, the pilot class, the comment marks.
- Labels: `hidden_cycle` (each hop imports the next file when the module loads, so the claim holds);
  `false_edge` (a hop does not import that file); `not_at_load` (a hop runs only in a script block, a
  dead `except` branch, or a TYPE_CHECKING block). FP = (`false_edge` + `not_at_load`) / read.
- Prediction: M4 rows 1,000 to 2,500 (pilot 1,652); FP 0% to 10%; marked recall 65% to 85% (pilot 76%).
  The likely result: bar B passes.

**B, mirrored_shapes.**
- B0, before any B code (Q3): 20 blind packets from the pilot pairs of section 4 (Rust snapshots and the
  Python annotated set). If S1 stops B0, do not build B; report to the owner.
- M1: the planted fixtures. M4: the snapshots of section 4 over an empty root; sample 60 rows, at most 12
  per snapshot.
- Packet: both type definitions with their files' paths and their docstrings; the shared names. Hidden:
  which type is new, the counts, the rule.
- Labels: `mirror` (one record in two homes; a field added to one must be added to the other by hand,
  and no compiler, generator or test ties them); `tied` (a compiler, generator or test fails on drift);
  `sibling` (two different things with the same member names); `generic` (the shared names are generic).
  FP = (`tied` + `sibling` + `generic`) / read.
- Prediction: FP 60% to 85% (the CO1B rate for sibling-prone overlap was 78.3%). The likely result: S1
  stops B0, and B is not built.

**Stop rules (both).** S1: after 20 findings, if the lower Wilson bound of the FP rate is above the bar A
rate, stop and report. S2: at most 60 findings. S3: stop when the measurement uses twice its estimate.

**Bars, written before the runs (Q6).** Bar A, hook advisory: FP <= 40% over >= 40 findings. Bar B,
block (erosion `CHECKS`, hook `block`, a gate-selftest row): FP <= 10%, Wilson upper bound <= 20%,
>= 60 findings, planted recall >= 50% per kind; for A also marked recall >= 70%.

## 9. Work units, order and effort

| Unit | Content | Gate | Estimate |
|---|---|---|---|
| U0 | refactor, no behavior change: the shared tree scan (6.6) and the two shared rows (6.5) | every suite and golden unchanged | 60k tokens, 0.5 day |
| U1 | facts: `Import.runs`, `Symbol.fields` (Python, Rust), FACTS_VERSION 12, fixtures | predicted fixture diffs; read goldens unchanged | 120k, 1 day |
| U2 | cycle_dodge: `import_chain`, core, shell, registry row, prompts, guide, M1 fixture | AGENTS.md command list | 150k, 1.5 days |
| U3 | A measurement: M1, M2, M4, readers, record in docs/analysis/ | bars of section 8 | 150k, 1 day |
| B0 | B pilot reading (no code) | S1 | 60k, 0.5 day |
| U4 | mirrored_shapes: `PairsSymbols`, core, shell, registry, prompts, guide, M1 fixtures | AGENTS.md command list | 180k, 1.5 days |
| U5 | B measurement | bars | 150k, 1 day |

- Order: U0, U1, U2, U3, then B0; U4 and U5 only if B0 passes S1. If B is deferred, U1 holds only
  `Import.runs`, and B's field takes its own bump later.
- No function above complexity 10 exists (C11), so no refactor is forced by that rule. U0 is forced by
  the parallel-ladder rule.
- Phase 2 (gate), only on a measured bar: bar A adds the check to `GatedCheck` as `advisory`; bar B also
  adds it to erosion `CHECKS`, `.agentless-mcp.json` `erosion_gate`, and scripts/gate-selftest.sh. Those
  commits need `Policy-Change:` trailers (AGENTS.md).

## 10. Risks

- The agentless Python resolver differs from the pilot resolver. M4 measures the product.
- Package `__init__.py` chains are 63% of the pilot dodges. Readers can find the fix wrong although the
  cycle is real. The label definition counts them as `hidden_cycle`.
- A gate on A blocks deliberate lazy imports in outside repositories. The gate is per project config.
- B floods with siblings, as callee_overlap did. B0 tests that before the build.

## 11. Owner questions

1. A's fact. Recommended: add `Import.runs` with one FACTS_VERSION bump. Reason: the guard keys on the
   invariant. The other choice (function spans, no bump) misses imports in methods of nested classes,
   which the extractor does not emit, and cannot see TYPE_CHECKING.
2. A's return path. Recommended: module-level imports only, as the brief defines the check. Reason: the
   row claims a load-time cycle, and that claim must hold. The other choice adds 360 pilot rows and 10
   of the 83 marked imports.
3. B before build. Recommended: run B0 (20 blind packets from the pilot) first, and build B only if S1
   does not stop it. Reason: CO1B stopped at S1 after the build; B0 costs about a third of U4.
4. B's Python fields. Recommended: annotated class-body names only. Reason: class-body assignments and
   `self.x` in `__init__` give 3,011 pilot pairs before the family limit, mostly siblings.
5. B cross-language pairs. Recommended: yes, with normalized names. Reason: a hand mirror across
   languages is the high-risk case of the boundary rule.
6. Bars. Recommended: the callee_overlap bars (section 8), plus marked recall >= 70% for A's bar B.
   Confirm the numbers.

## 12. Owner answers (2026-11-05)

The owner approved this specification and the recommended answer of each question of section 11.

- Q1. A's fact: `Import.runs` (`load`, `call`, `type_only`), with one FACTS_VERSION bump, 11 to 12.
- Q2. A reports only a cycle that closes when the local import moves to module level. A reports no
  cycle that closes only through other local imports.
- Q3. B (`mirrored_shapes`) is not in the first build. A blind paper test (B0) decides later whether
  B gets built. Thus U1 adds `Import.runs` only, not `Symbol.fields`, and B's field takes its own
  FACTS_VERSION bump later (section 9).
- Q4 and Q5 are B's answers. They apply only if B0 passes and B gets built.
- Q6. The bars of section 8 hold as recommended. They belong to the measurement unit U3.
