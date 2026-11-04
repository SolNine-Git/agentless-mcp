# RSC: Rust scope analysis for references

Ticket RSC. Status: specification for the owner. No product code changed. Base: main 81d5a7e.
Author: the RSC agent, 2026-11-04. Work directory: /var/tmp/claude-rust-port/rsc (W).

Every figure in this document is labeled **measured** (with the command or the script) or
**derived** (computed from measured data, with the method). "Site" means one identifier
occurrence (one `Ref`). "Row" means one refs line (one (path, line) pair).

## 1. Summary

Rust references bind to definitions by name, with no scope check and no definition-kind check.
This specification makes Rust a language with scope analysis, at parity with Python, and adds a
definition-kind rule for bare names. The change has five rules:

| Rule | What it does | Where |
|---|---|---|
| R1 locals | A Rust pattern binding gets the role `binding`. A use of a visible local gets `local`. Neither binds. | facts |
| R2 members | A field name (`x.f`, `Foo { f: v }`, `struct S { f: T }`, `S { f: p }`) and a method name (`x.f()`) get `attribute`, as in Python. They bind nothing. | facts |
| R3 kind | A Rust bare name binds only a definition with no parent (not a method), at the import tier and at the name-only tiers. The same-file tier already has this rule. | graph resolver |
| R4 type paths | The last segment of a path whose qualifier is a type (`T::f`, `Self::f`, `<T as Tr>::f`, `Vec::<T>::new`, `Piece::Text`) gets `attribute`. It binds nothing. | facts |
| R5 declarations | The name of an item or an enum variant that is not a symbol (an enum variant, a nested `fn`, a `const` in a body, a `mod` name) gets `declaration`. It binds nothing. | facts |

Measured result of R1 to R4 with a prototype (R5 is not in the prototype): the binding-tier Rust
sites fall by 5,315 of 44,282 (this repository), 2,162 of 8,905 (ripgrep-2576) and 2,722 of
15,996 (tokio-6752). The prototype adds no binding-tier site. The four Rust false-edge findings of
CO1B lose their false callee. The cost: 6 Rust callee_overlap rows that the CO1B readers labeled
"duplicate" (3) or "sibling" (3) also go, because their shared callees were method calls that bound
only through a scope defect (sections 2 and 3.5). A Rust method also loses its fan-in edges in `explain`,
as a Python method has none today.

## 2. The problem, reproduced

**Evidence 1, measured.** `agentless-mcp refs --repo W/subj --json --limit 500
rs:crates/app/src/verdicts.rs::record` (frozen CLI /var/tmp/claude-rust-port/co1b/bin/final, main
81d5a7e) lists 234 rows. 26 rows are at the same-file tier, in one group marked "locals not
checked". I read all 26 lines in the source. 1 row (line 62, `.map(|(position, line)| record(line,
position))`) is a call of the function. 25 rows are the parameter `record: &Pairs` (lines 114, 135,
179, 196, 204, 212, 221, 240), the `for (position, record)` pattern (line 88), or a use of one of
them. The brief said "at least 13". The true count is 25 of 26.

**Evidence 2, measured.** The second mechanism is a scope defect in the import scopes, not a local.
`mod tests { use super::*; }` sits at inline depth 1. `RustTree::resolve_in_file`
(crates/graph/src/rust_paths.rs:164-184) answers the importing file itself for a bare `super`
(line 172). `bind_leaf` (crates/graph/src/scopes.rs:325-327) puts that file into its own
`wholesale` set. `Resolver::bound_here` (crates/graph/src/resolver.rs:233-251) then binds every name
of the file to every same-file definition of that name through `scope.binds`, and that branch has no
top-level filter. The same-file branch above it has one (`in_module_scope`, resolver.rs:363-366,
"A bare name cannot reach a class member"). So in a file with `mod tests { use super::*; }`, every
bare name, field name and method name binds to every method of that name in the file, at the
resolved-via-import tier. Both files of CO1B's tokio and ripgrep findings have this line
(tokio/src/sync/broadcast.rs:1508, crates/searcher/src/searcher/mod.rs:1023). Python does not have
the defect: `bind_statement` drops a self-import (`target.filter(|target| *target != path)`,
scopes.rs:471).

**Evidence 3, measured.** The CO1B packets szri, u859 (the parameter `record`), nxkq (`drop(tail)`
and `drop(slot)` bound to 5 `Drop::drop` methods) and vwfd (`heap_limit`, a field, and
`line_terminator`, a method of another type, bound to `SearcherBuilder.heap_limit`,
`Searcher.line_terminator` and `SearcherBuilder.line_terminator`) show both mechanisms.

**Two guide claims are false for Rust today.** The guide says "A call through a selector is an
attribute member, which resolves to no edge at any tier" (crates/app/data/agent-guide.md:1137,
and the comment at crates/app/src/health.rs:21-22). It also says "a Rust call on a receiver
(`x.m()`) ... never count[s]" for callee_overlap (agent-guide.md:1607). In Rust, a method name is a
`reference` and binds (sections 3.1 and 3.3). R2 and R3 make both claims true.

## 3. Measurements

### 3.1 Instrument

- Binary: W/bin/measure/agentless-mcp, sha256 a500e506051d1a14... (full sum in
  W/bin/measure/SHA256SUMS). It is main 81d5a7e plus W/scripts/rsc-measure.patch (never committed).
  The patch adds a prototype of R1, R2, R4 (crates/facts/src/rsc_proto.rs, gated by
  `AGENTLESS_RSC_PROTO`), a prototype of R3 (crates/graph/src/resolver.rs, gated by
  `AGENTLESS_RSC_R3`), and a dump hook in `cycles` (crates/app/src/rsc_dump.rs, gated by
  `AGENTLESS_RSC_DUMP`).
- Proof that the patch is neutral when off, measured: `refs --json` on `verdicts.rs::record` and
  `explain` on `verdicts.rs::vote_candidate` from the measure binary and from the frozen CLI are
  byte-equal (`cmp`). Rerun after each of the 3 builds.
- Trees: this repository at 81d5a7e (W/subj, a clean shared clone), and copies of the bench
  snapshots burntsushi__ripgrep-2576 and tokio-rs__tokio-6752 (W/repos).
- Dump command, measured: `AGENTLESS_RSC_DUMP=W/dump/<tree>.jsonl agentless-mcp cycles --repo
  <tree>`. The dump re-parses each Rust file, aligns the parse nodes with the stored refs (0
  alignment failures in all 3 trees), classifies each site by its syntax, and resolves every site
  under the base and under 6 variants. Counts: W/scripts/count.py; output W/out/count3.txt.
- Every run: `XDG_CACHE_HOME`, `AGENTLESS_GATE_LOG` and `AGENTLESS_EROSION_LOG` under W
  (W/scripts/am.sh, W/scripts/mm.sh). "is relative and was ignored": 0 in every output.

### 3.2 Binding-tier Rust sites by class, base (measured, count.py on the dumps)

| Class | this repo | ripgrep | tokio |
|---|---:|---:|---:|
| All binding-tier sites (same-file + resolved-via-import) | 44,282 (17,941 + 26,341) | 8,905 (5,267 + 3,638) | 15,996 (8,652 + 7,344) |
| (a) parameter or local: the binding and its uses | 3,341 | 541 | 667 |
| (b) field access `x.f`, and a `.f` token in macro arguments | 316 | 449 | 528 |
| (b2) other field names: initializer, declaration, pattern | 274 | 153 | 79 |
| (c) bare call or name whose candidates include a method | 41 | 4 | 72 |
| (d) receiver method call `x.f()` | 473 | 622 | 824 |
| type-qualified path tail `T::f` (all candidate kinds) | 1,047 | 353 | 493 |
| other classes (types, bare calls of functions, use leaves, module paths, macro tokens) | rest | rest | rest |

Cross-check, derived: the graph-fidelity commit f7b0d89 recorded ripgrep imported 3,638 and tokio
imported 7,356 at its base. The dump gives ripgrep 3,638 and tokio 7,344 at 81d5a7e.

### 3.3 Seeded samples read in the source (seed 20261104, W/scripts/sample.py)

- (a) locals, 28 rows (12 this repo, 8 tokio, 8 ripgrep): 28 of 28 are parameters, closure
  parameters, `let` bindings or their uses. Example: crates/io/src/read.rs:218 `let tree =
  tree();` keeps the call `tree()` (evaluated before the binding) and drops the binding.
- (b) field access, 11 rows: 11 of 11 are member names that bound to a function or a method of the
  same name: 10 field reads (`self.config.multi_line`, `found.rows`, `self.seq`) and 1 method call
  inside a macro (`assert_eq!(rdr.bstr(), ..)`; the dump's class does not split a `.f` token in
  macro arguments into field and method, so class (b) holds such calls).
- (b2) field names, 5 rows: 5 of 5 are struct field declarations or initializers (`pub cycles:
  Bounded<CycleRow>,`).
- (c) bare name to method, 11 rows: `drop(x)` or `.map(drop)` bound to `Drop::drop` methods (4),
  the path head `fmt` of `impl fmt::Display for X` bound to `X.fmt` (4), `spawn_local(..)` bound
  to `LocalSet.spawn_local` (1), a closure parameter `error` inside `assert_eq!(..)` bound to
  `Reader.error` (1; a closure in macro arguments is a token tree, so R1 cannot see the
  parameter), `blocks(..)` with the candidates `Measured.blocks` and the free `blocks` (1). 10 of
  11 are false. For `blocks`, R3 drops the method and keeps the free function at the same tier.
- (d) method calls, 14 rows. Same-file rows bind a free function of the same name: `lock.try_write()`
  to a test function `try_write`, `rt.spawn_blocking(func)` to the free `spawn_blocking`,
  `.line_number(true)` to a free `line_number`. All false. Imported rows come only from the
  self-glob of section 2 and often name the right method by accident: `self.at_quotes(..)` to
  `Scanner.at_quotes`, `self.counted(..)` to `Walker.counted`.
- type-qualified path tails kept by R1 to R3, 15 rows: `Piece::Text(..)` (an enum variant) bound to
  the struct `template.rs::Text` (707 sites in this repo), `Self::Captures` (an associated type)
  bound to the trait `Captures`, `Ready::WRITABLE` (an associated const) bound to a module const
  `WRITABLE`, `AsyncReadExt::read_to_string` bound to a test function. In 15 of 15, the bound
  definition is not the item that the path names.

Owner match of the self-glob rows, derived (W/scripts/owner_check.py): of the binding sites that R3
removes, the receiver is `self` or the path is the owner type, and a candidate method has that
owner, in 232 (this repo), 280 (ripgrep) and 244 (tokio) sites. These are true edges that exist
only by accident, in files that hold `mod tests { use super::*; }`. The same calls in a file
without that line bind at no binding tier today.

### 3.4 Effect of each rule on the binding tiers (measured, count3.txt)

Variants are cumulative. v6 is the design of section 4 without R5.

| Variant | this repo | ripgrep | tokio |
|---|---:|---:|---:|
| base | 44,282 | 8,905 | 15,996 |
| v1 = R1 locals | 40,941 (-3,341) | 8,364 (-541) | 15,329 (-667) |
| v2 = v1 + R3 at the import tier | 40,495 (-446) | 7,284 (-1,080) | 13,890 (-1,439) |
| v3 = v2 + R2 field names | 39,983 (-512) | 7,075 (-209) | 13,804 (-86) |
| v4 = v3 + R2 method names | 39,782 (-201) | 6,829 (-246) | 13,382 (-422) |
| v5 = v4 + R4 type-path tails | 38,967 (-815) | 6,743 (-86) | 13,274 (-108) |
| v6 = v5 + R3 at the name-only tiers | 38,967 | 6,743 | 13,274 |
| Sites that gain a binding tier, any variant | 0 | 0 | 0 |

Split of the R3 removals at the import tier, derived: the self-glob gives 387 of 446, 1,080 of
1,080 and 1,047 of 1,439. Other globs (`use super::*` in a submodule file) give 59, 0 and 392.

Name-only tiers and the file graph (measured, aggregate rows of the dump). `is_reference` counts
the sites that the file graph of `map`, `communities` and `diagram` reads.

| Tree | Tier | base | v1 | v6 |
|---|---|---:|---:|---:|
| this repo | unique / ambiguous | 19,790 / 77,521 | 10,166 / 31,461 | 2,493 / 2,840 |
| this repo | is_reference | 231,894 | 156,790 | 98,864 |
| ripgrep | unique / ambiguous | 8,014 / 10,741 | 6,610 / 7,257 | 786 / 532 |
| ripgrep | is_reference | 54,401 | 41,750 | 25,800 |
| tokio | unique / ambiguous | 20,882 / 33,215 | 15,249 / 27,410 | 7,561 / 6,008 |
| tokio | is_reference | 184,787 | 145,283 | 107,352 |

### 3.5 callee_overlap on the CO1B reading set (measured, W/scripts/co1b_compare.py)

Command per tree, as CO1B ran it: `lint --repo <tree> --since <root> --only callee_overlap --json`
for the M4 copies (W/co1b-trees/tmy4pwit, tq26x8nv, t53fhz5z), and `--since <merge-base>` for
pr64 and pr66 (my own worktrees at their heads). Prototype run: `AGENTLESS_RSC_PROTO=15
AGENTLESS_RSC_R3=1` (R3 level 2 changes no binding tier, so the row set is the v6 row set).

- Base rows equal CO1B's M4 rows: 32, 11 and 10 of 32, 11 and 10.
- The 33 Rust rows of the reading set (self 10, ripgrep 10, tokio 10, pr64 1, pr66 2), joined with
  the labels of both readers (W/out/co1b_labels.txt):

| Reader label (both readers agree on all 33) | Rows | Kept | Gone |
|---|---:|---:|---:|
| false_edge | 16 | 1 (u859, without `record`) | 15 |
| duplicate | 5 | 2 | 3 (yza9, jn7h, nz7h) |
| sibling | 12 | 9 | 3 (pz5e, nz7r, vizr) |

- The four findings of the brief: szri row gone, u859 row kept and its false callee `record`
  removed, nxkq row gone (5 false `drop` callees removed), vwfd row gone (3 false callees removed).
- The 6 lost true rows share callees that bound only through the self-glob: tokio
  `LinkedList.pop_back ~ remove` shares `Entry.from_raw`, `Entry.pointers`, `Pointers.set_*`; these
  are the test module's `Entry` impl, not the definitions that `L::from_raw` calls. The readers
  labeled the pair by its bodies ("duplicate" takes precedence over "false_edge").
- Rows that only the prototype gives, in the self tree, not in the reading set (not labeled):
  `validate ~ patch_step` (crates/cli/src/commands/write.rs:86), `assignment ~ variable`
  (crates/facts/src/extract/lua_assignments.rs:33), `text_or_null ~ int_or_null`
  (crates/hook/src/report/field.rs:52). Rows gone outside the reading set (not labeled): in the
  self tree `pack ~ auto_budget`, `string ~ boolean`, `integer ~ boolean`, `vote ~ patch`,
  `patches ~ pairs`, `Reader.add ~ Reader.open`, `check_choices ~ value_order`, `run ~ kill_group`
  (8 of 11 gone); in ripgrep `LineBuffer.fill ~ LineBuffer.clear` (1 of 9 gone); in tokio none
  (0 of 9 gone).
- Planted pairs, measured: tests/characterization/fixtures/callee_overlap_rust and _py, rebuilt as
  the test builds them (base commit, change on top), `lint --since HEAD`: the `candidates` JSON of
  the base and the prototype are equal for both languages (10 rows each). The golden
  callee_overlap_rust.lint.json is predicted unchanged.

### 3.6 refs and explain, preview (measured, W/scripts/refs_preview.py)

40 seeded targets per tree (definitions that a binding-tier site lands on) plus the four named
targets. `refs --json --limit 500`, base against `AGENTLESS_RSC_PROTO=15 AGENTLESS_RSC_R3=2`.

| Tree | Answers changed | Row moves |
|---|---|---|
| ripgrep | 18 of 42 | imported->ambiguous 58, same_file->ambiguous 16, unique->ambiguous 9, ambiguous->unique 1 |
| tokio | 10 of 41 | imported->ambiguous 77, unique->ambiguous 18, same_file->ambiguous 2 |
| this repo | 8 of 41 | same_file->ambiguous 32, unique->ambiguous 14, imported->ambiguous 7 |

`total` is equal in every answer. Two rows of `hook/src/text.rs::shown` are absent in the prototype
answer only because the answer is over the token ceiling and drops whole groups in both builds.
`explain rs:crates/searcher/src/searcher/mod.rs::Searcher.line_terminator` (ripgrep): fan-in 27
(3 resolved-via-import, 24 name-only-ambiguous) becomes none; fan-out 3 becomes 1. `health` on
ripgrep: orphans 7 and unused exports 11 do not change; hubs 1,725 become 1,629.

### 3.7 Loc-Bench retrieval tier (measured, W/scripts/locbench_maps.py)

- 10 of the 144 checkouts in loc-bench-harness/repos hold `.rs` files (find). They serve 10 of the
  150 instances of results/fix-qualifier.jsonl: ctc-oss__fapolicy-analyzer-809 (8 gold `.rs`
  files), 3 sqlglot, 3 meson, 3 ultralytics (no gold `.rs` file).
- `map --granularity file --max-files 20 --json`, with the record's seeds and with none, on copies
  under W/locbench, base against each prototype mode: with R1 only, 1 seeded and 3 no-seed lists
  change; with R1, R2 and R4, 2 seeded (fapolicy-809, ultralytics-17728) and 6 no-seed lists
  change. recall@10 is equal in all 20 runs in all modes. The other 140 instances hold no Rust file,
  so their facts, and their lists, cannot change (derived from the code: every rule reads only
  Rust parses and Rust files).

### 3.8 Fixture predictions (derived from prototype runs)

- crates/facts/fixtures/extract_rust.json: each case source written to a file and dumped. 15 of 16
  cases change. 184 of 538 ref rows leave the role `reference` (R1, R2, R4; R5 adds more).
- crates/store/src/tests/blobs.rs: the Rust sample text holds no local, field, method or type
  path. Its refs do not change (dump: equal `is_reference` counts). The digest stays; PINNED goes
  from (10, digest) to (11, digest).
- tests/characterization/fixtures/tier1 and callee_overlap_rust: no binding-tier site moves;
  `unique` 1 -> 0 and 4 -> 0; `is_reference` 41 -> 31 and 520 -> 253.

## 4. Decisions

### D1. Scope rules for Rust

Model. The rules follow rust-analyzer `ExprScopes` (crates/hir-def/src/expr_store/scope.rs: a `let`
pattern is added to a new scope after its initializer and else block; a closure, a match arm and
an `if let` then-branch each open a scope) and the rustc rib rule (rustc-dev-guide "Name
resolution": a nested function item does not see the locals of the outer function). The analysis
needs only booleans: a site is a local or it is not. Which local it names does not matter.

Binding sites, and where their names are visible (byte ranges of the parse):

| Binding site (node, field) | Visible in |
|---|---|
| `function_item.parameters`, each `parameter.pattern` and `variadic_parameter.pattern` | the function `body` |
| `function_signature_item.parameters`, `function_type.parameters` | nowhere (the names are bindings, never references) |
| `closure_expression.parameters`, each pattern or `parameter.pattern` | the closure `body` |
| `let_declaration.pattern` | from the end of the `let` statement to the end of the enclosing `block` (not the initializer, not the `else` block) |
| `let_condition.pattern` (`if let`, `while let`, let chains, a match guard) | from the end of the condition to the end of the guarded block: `if_expression.consequence`, the `while_expression`, or the `match_arm`; never the `else` |
| `match_arm` pattern (the `match_pattern` child that is not `condition`) | the guard and the arm value |
| `for_expression.pattern` | the loop `body` |

Names that a pattern binds: the pattern itself when it is an `identifier`; the children of
`tuple_pattern`, `slice_pattern`, `or_pattern`, `ref_pattern`, `mut_pattern`, `reference_pattern`,
`captured_pattern` (the name before `@` and the sub-pattern) and `parameter`; the children of
`tuple_struct_pattern` and `struct_pattern` except the `type` field; `field_pattern.pattern`, or a
`shorthand_field_identifier`. Not a binding: an identifier that starts with an ASCII upper-case
letter (by Rust convention a constant, a unit struct or a unit variant), a `scoped_identifier`, a
range bound, a literal, a macro pattern. Each identifier of a pattern that is not a binding is
pattern syntax and is never a local.

Uses. An `identifier` is a use of a local when a binding of the same name is visible at its start
byte and both have the same owner. The owner is the innermost enclosing item of kind
`function_item`, `function_signature_item`, `impl_item`, `trait_item`, `mod_item`, `const_item`,
`static_item`, `struct_item`, `enum_item`, `union_item`, `type_item`, `macro_definition` or
`foreign_mod_item`. A closure, a block, an `async`, `unsafe` or `const` block is no owner, so its
body sees the enclosing locals. An identifier is never a local when it is: a segment of a
`scoped_identifier` or `scoped_type_identifier` path, a use-tree part, the `macro` field of
`macro_invocation`, the `name` of an item or an `enum_variant`, or inside an `attribute_item`,
`inner_attribute_item` or `macro_definition`. A `type_identifier` and a `field_identifier` are never
locals (types and fields are other namespaces).

Shadowing order. A binding is visible only from the start of its range, so `let x = x;` keeps the
right side's `x` as it was, and `let tree = tree();` keeps the call. A later binding of the same
name does not change the role of a site. A local shadows an item of the same name in the value
namespace (Rust semantics), so a call `f(x)` where `f` is a visible local closure is a local.

Macros.
- The token tree of a macro invocation (`name!(...)`): macro_rules hygiene resolves the caller's
  tokens at the call site, so a token that names a visible local is a local. A token right after
  `.` is a member name (R2). A token after `::`, or before `::` or `!`, is path syntax and keeps
  its role.
- A `macro_rules!` body is a template. Its tokens keep the role `reference`. No scope analysis
  runs inside it.
- An attribute (`#[...]`) keeps its tokens as they are.

One home. The kind lists of this section are data in crates/syntax/languages/rust.toml, under
`[extraction]`, beside `identifiers`: `scope_owners`, `opaque_scopes` (attribute and macro
definition kinds), `nested_patterns`, `typed_patterns` (the patterns whose `type` field is no
binding) and `path_parents`. The generator emits them into `ExtractionTable`, `Extraction` resolves
them to `KindSet`s, and the generator refuses them for a role rule other than `rust`, as it already
refuses `builtins` for a role rule other than `python` (xtask/src/generate/syntax.rs:283). A test
checks that each listed kind exists in the Rust grammar. The ranges of the binding-site table are
behavior, so they stay as one keyed table in a new module crates/facts/src/extract/rust_scopes.rs.
Reason: the generator already carries and checks kind lists, and a misspelled kind in a `KindSet`
matches nothing silently (`Language::kinds` drops unknown names), so the lists need the check. The
alternative, consts in rust_scopes.rs as in python_scopes.rs, is question Q4.

### D2. Definition kinds

| Site | Today (81d5a7e) | After |
|---|---|---|
| bare call `f(x)` | top-level `f` of the file (same-file); an imported `f`; through a glob or the self-glob, every definition named `f`, methods included (imported); else name-only over every definition named `f`, methods included | a visible local closure: no binding (`local`); else a top-level definition: a free function, a tuple struct (its constructor), a constant, a static, an imported item. Never a method, at any tier (R3). An enum variant is no symbol, so `Variant(x)` binds only when a struct of that name exists, as today |
| bare name `f` (no call) | as the bare call | as the bare call |
| field access `x.f` | as a bare name: a same-file free function `f`, an imported `f`, any same-file method `f` through the self-glob, else name-only | `attribute`: no binding, no edge at any tier (R2). refs lists it as a name-only-ambiguous spelling |
| receiver method call `x.f()` | as a bare name (same as field access) | `attribute`, as Python's `self.f()`: no binding, no edge (R2) |
| type path `T::f()`, `Self::f()`, `<T as Tr>::f()`, `Vec::<T>::new()`, `Enum::Variant` | the tail has no qualifier (`module_like` refuses an upper-case segment), so it resolves as a bare name, with the self-glob leak to methods | `attribute`: no binding (R4). The head `T` keeps binding the type, so a caller of `T::f` still has an edge to `T` |
| module path `m::f()`, `crate::m::f()`, `super::f()` | the qualifier walk of graph-fidelity (#65): binds `f` in the module file or through its re-exports, else the fallback to the bare name | unchanged, except that the fallback to the bare name now applies R3 |
| `use` leaf | `reference`, binds at resolved-via-import | unchanged |
| item or variant name not caught by `Declared` | `reference` (decl-name class: 74, 23 and 71 binding sites, measured) | `declaration` (R5): no binding; a line that only declares leaves refs, as in Python |

This work does not bind `self.f()`, `Self::f()` or `T::f()` to `T.f` by owner. That rule (R6)
would add binding edges, not remove them. It is question Q2.

### D3. The tier result

- Rust counts as a language with scope analysis. `capabilities` prints `scope analysis (a
  parameter or local is told from a caller): python, rust`, and its JSON field
  `scope_analysed_languages` gains `"rust"` (an added value; the field is unchanged).
- The knowledge gets one home: a method `RoleRule::analyses_scopes()` in crates/syntax (true for
  `Python` and `Rust`). `scope_analysed` (crates/app/src/refs/sites.rs:95-97, `language ==
  Language::Python`) and the literal `["python"]` (crates/app/src/capabilities.rs:231) both read it.
  `Capabilities::scope_analysed_languages` becomes `Vec<&'static str>` (a public API change of
  `app`; crates/app/public-api.txt).
- A Rust binding-tier group gets `scope_checked: true`, so its header loses ", locals not checked"
  and its JSON loses `"scope_checked": false` (render/src/refs.rs:135-136, 356). The tool
  description sentence "A group marked 'locals not checked' comes from a language that has no scope
  analysis" stays true and does not change.
- A removed binding does not leave refs. refs lists every spelling of the target's name
  (`references_to`, crates/graph/src/lookup.rs:163-175), so the row stays, at the
  name-only-ambiguous tier (`site_tier` gives `Ambiguous` to a site that binds nothing,
  refs/sites.rs:134-136). This is the Python behavior today. `total` does not change; `bound_total`
  falls. Exceptions: a line that only declares (R5) leaves refs, and a row can move up from
  name-only-ambiguous to unique when R3 removes the method candidates (measured: 1 row in the
  ripgrep preview).
- In `explain`, `graph`, `lint` and `health`, a removed binding is gone: it forms no edge at any
  tier (`resolved_sites` skips a role that `is_resolvable` refuses, resolver.rs:181).

### D4. Facts

The change alters the stored facts: the `role` of Rust refs. No field and no role value is new.
`FACTS_VERSION` goes from 10 to 11 (crates/store/src/analysis.rs:18). The blob test pin moves to
(11, the same digest) (section 3.8). The change of rust.toml also moves `TABLES_DIGEST`, which is in
the analysis key, so every stored index is rebuilt once on the first call. The Python, Go,
TypeScript and other facts do not change.

### D5. Who sees the change

| Surface | Change |
|---|---|
| `refs` (find_referencing_symbols) | Rust rows move from same-file, resolved-via-import and unique to name-only-ambiguous (locals, members, type-path tails, bare names whose only candidates were methods); a few move up to unique; declaration-only lines leave (R5); Rust groups lose "locals not checked"; `--shared-callers` stops ranking locals and members (refs/shared.rs:101 reads `is_reference`) |
| `explain` | Rust fan-out loses the local, member and type-path rows; a Rust method's fan-in falls to none or near none, as a Python method's today |
| `graph path` | paths over false member and local edges go; a path can get longer or become "no path" |
| `graph communities`, `diagram`, `html` | the file graph reads `is_reference` (crates/graph/src/files.rs:181): Rust locals and member names stop adding file edges, so weights, communities and the diagram of Rust trees change |
| `graph health` | hubs: Rust methods and types lose counted degree (ripgrep 1,725 -> 1,629 hubs, measured); orphans and unused exports are functions only (health.rs:21-23), measured unchanged on ripgrep; a function whose only callers were shadowing locals becomes an orphan candidate (a true finding) |
| `graph cycles` | unchanged: module imports do not read roles |
| `map` ranking | the file graph change moves PageRank on trees with Rust files (section 3.7); `map` companions read `is_reference` (crates/app/src/map/companions.rs:189) |
| `lint callee_overlap` | Rust callee sets lose false callees (section 3.5) |
| `lint dangling_callers` | a Rust local or member that spells a removed name no longer counts as a caller (crates/patchcheck/src/callers.rs:63) |
| `lint` dangling_references, arity, undeclared_imports | unchanged: Python only |
| `lint` shadowing, near_duplicates, cycle_delta | unchanged: symbols and imports only |
| erosion gates (complexity_delta, contract_drift, clone_delta, `cargo xtask erosion`) | unchanged: they read syntax metrics and the health complexity figures, not roles (xtask/src/erosion.rs:343-356) |
| `capabilities` | the scope line and the JSON list gain rust |

### D6. Measurement plan (the release regression gate of docs/analysis/benchmark-methodology.md)

1. **Suites.** Every command of AGENTS.md, plus `scripts/gate-selftest.sh "$gate"` (the change
   touches xtask/). Predicted fixture and golden diffs, and nothing else:
   - crates/facts/fixtures/extract_rust.json: the roles of about 184 of 538 rows in 15 of 16 cases
     (derived), plus the R5 rows, plus the new cases of phase 1.
   - crates/store/src/tests/blobs.rs: PINNED (11, unchanged digest).
   - crates/app/fixtures/capabilities.json, crates/mcp/fixtures/calls.json (7 strings),
     crates/cli/fixtures/scenarios.json (2 strings): "python" -> "python, rust", and the JSON list.
   - crates/app/public-api.txt: the type of `scope_analysed_languages`.
   - crates/graph/fixtures/corpora/tier1.json and fixtures_tree.json hold recorded facts (no Rust
     role changes there). Only R3 at the name-only tiers can move them: the rows of the 3 and 5
     bare spellings of the Rust method names `new`, `price`, `drain`, `next` (derived from the
     corpora). Any other corpus row that moves is a defect.
   - crates/syntax generated tables and `TABLES_DIGEST`.
   - No change: tests/characterization/goldens (graph, lint, map, complexity), tokens.json, the
     render capabilities fixture (an input), crates/app/fixtures/guide, the prompts.
2. **Retrieval tier.** Frozen copy, `cargo build --release --locked -p agentless`, `loc-bench run
   --agentless-bin`, seeded and no-seed arms. Base: re-run 81d5a7e once and byte-diff it against
   the stored arm. Prediction: 140 of 150 instances byte-identical in each arm; the changed lists
   are among the 10 Rust-holding instances and equal the prototype lists in
   W/out/locbench_maps.json (2 seeded, 6 no-seed). R5 is not in the prototype and can move a list
   of those 10 further; report each such list. A changed list outside the 10 is a defect. Then the paired bootstrap on both arms with
   recall@10, as the methodology requires for any difference.
3. **Surface diff.** `surface_dump.py` and `surface_cmp.py` over the Loc-Bench checkouts, plus the
   3 Rust trees (this repo at the build's base, ripgrep-2576, tokio-6752). Report per surface the
   changed answers, the refs tier transitions keyed by (path, line), a loss count, and a sample of
   changed lines read in the source. The written prediction (falsifiable):
   - Python, Go and TypeScript trees: 0 changed answers on every surface.
   - refs: `total` equal in every answer; every downward move ends at name-only-ambiguous; every
     moved row's site is a local, a member name, a type-path tail, a declaration (R5) or a bare
     name whose candidates were methods; up-moves only from name-only-ambiguous to unique; no row
     gains a binding tier.
   - explain: fan-in of Rust methods falls; no Rust fan-in or fan-out row gains a binding tier.
   - cycles: 0 changed answers.
   - The loss count of refs bound rows is the class counts of section 3.4, within the R5 rows.
   Mask the receipt's cache field. Mark the answers that carry `truncated`.
4. **callee_overlap re-run on the CO1B reading set.** The same commands as section 3.5 with the
   build, on the 5 Rust trees and on every Python tree of the set. Pass: szri, nxkq and vwfd give
   no row; u859 gives its row without `record`; each other change equals the list of section 3.5
   (3 duplicate and 3 sibling rows lost; 3 new rows in the self tree, which a reader labels before
   release); the Python rows and the planted goldens do not change.
5. **Exposure check.** On the 2026-09-27 hooked arm, `find_referencing_symbols` ran 10 times in 9
   instances and `symbols(explain)` 5 times in 5; the graph operations did not run. An agentic run
   cannot see this change. Recommendation: no paid run for this ticket.

### D7. Phases

Each phase is one commit or a short series, each green on the suites. No function that a phase
edits is above cyclomatic complexity 10 (measured: `health --json` on 81d5a7e gives
`over_threshold: 0`; `Resolver.resolve` is at 6), so no phase needs a refactor-only commit by the
complexity rule. P0 is still a refactor with no behavior change, because it gives the scope flag
its one home before P1 flips it.

| Phase | Content | Files | Estimate |
|---|---|---|---|
| P0 one home, no behavior change | `RoleRule::analyses_scopes()` (Python only); sites.rs and capabilities.rs read it; `scope_analysed_languages: Vec<&'static str>`; render test row type | crates/syntax/src/extraction.rs, crates/app/src/refs/sites.rs, crates/app/src/capabilities.rs, crates/render/src/tests/rows.rs, crates/app/public-api.txt | 30k tokens, 1 h |
| P1 Rust roles (R1, R2, R4, R5) | the kind lists in rust.toml and the generator; rust_scopes.rs; identifier_refs.rs runs it for `RoleRule::Rust` beside `RustPaths`; `analyses_scopes()` true for Rust; FACTS_VERSION 11; fixtures; tests | crates/syntax/languages/rust.toml, crates/syntax/src/tables.rs, crates/syntax/src/extraction.rs, crates/syntax/src/generated/*, xtask/src/generate/syntax.rs, crates/syntax/src/table_tests.rs, crates/facts/src/extract/rust_scopes.rs (new), crates/facts/src/extract/identifier_refs.rs, crates/facts/src/extract.rs, crates/store/src/analysis.rs, crates/store/src/tests/blobs.rs, crates/facts/fixtures/extract_rust.json, capabilities fixtures | 150k tokens, 1.5 days |
| P2 definition kind (R3) | one predicate "a Rust bare name reaches only a definition with no parent", applied in `bound_here`'s import branch and in `resolve`'s name-only list, in a named function | crates/graph/src/resolver.rs, crates/graph/src/tests/rust.rs, corpora if predicted | 40k tokens, 0.5 day |
| P3 measurement | D6 steps 1 to 4, a reader for the 3 new callee_overlap rows, the CHANGELOG measured figures | W only, then CHANGELOG.md | 120k tokens, 1 day (the retrieval tier is about 12 min per arm) |

Tests to add:
- rust_scopes: one test per row of the D1 tables and per macro rule: a parameter used in the
  body; a closure parameter; `let x = x;`; `let tree = tree();`; `let ... else`; `if let` with an
  `else` that uses the name; `while let`; a let chain; a match guard; `for (i, x)`; a nested `fn`
  that uses the outer name (still a reference); a closure that uses the outer name (local); a
  shorthand field initializer; an upper-case pattern (`MAX =>` stays a reference); a macro argument
  `dbg!(record)` (local), `assert!(a.f)` (`f` attribute), `vec![m::f()]` (path); a
  `macro_rules!` body; `x.f`, `x.f()`, `x.f::<T>()`, `T::f()`, `Self::f`, `<T>::f`,
  `Enum::Variant`; an enum variant name (declaration).
- rust.toml: every new kind exists in the grammar; the generator refuses the lists for another role
  rule.
- graph resolver: in a file with `mod tests { use super::*; }`, `drop(x)` and `x.heap_limit` bind
  no method at any tier; a bare call of a top-level function in that file still binds same-file;
  a glob `use super::*` in a submodule file binds the parent's functions, not its methods.
- app refs: the verdicts.rs shape (a free `record` and a parameter `record`): `bound` 1, the
  parameter rows at name-only-ambiguous, the Rust group `scope_checked` true.
- capabilities: the list comes from the role rule and holds python and rust.
- tests/characterization/fixtures/callee_overlap_rust: add 2 planted negatives, the parameter
  shadow (szri shape) and `drop` through the self-glob (nxkq shape). The golden rows do not change
  (a negative gives no row); pairs.json gains the negatives.

Prompts and guide: no prompt text changes. The guide needs no edit: its two claims of section 2
become true for Rust. If the owner wants a sentence, the refs section can say "Python and Rust tell
a parameter or local from a caller" (that edit changes crates/app/fixtures/guide/guide.json and
the cli guide scenarios).

CHANGELOG entry (draft for "Unreleased"; P3 fills the measured figures):

```
### Changed

- **Rust has scope analysis: a parameter, a local or a member name is no
  reference.** A Rust pattern binding (a function or closure parameter, `let`,
  `if let`, `while let`, a match arm, `for`) and each use of it in scope are no
  longer references, as in Python, so `refs` lists them at the
  name-only-ambiguous tier and no edge comes from them. A field name, a method
  name of `x.f()`, and the last segment of a type path (`T::f`, `Self::f`,
  `Enum::Variant`) bind nothing, as a Python attribute does. A bare name binds
  only a definition that is not a method, at every tier. `capabilities` lists
  rust under scope analysis, and a Rust refs group is no longer marked "locals
  not checked". FACTS_VERSION 10 to 11: every stored index is rebuilt once.
  Measured with a prototype on this repository, ripgrep-2576 and tokio-6752:
  binding-tier Rust sites 44,282 -> 38,967, 8,905 -> 6,743 and 15,996 ->
  13,274, none gained. <the build's figures, the retrieval tier and the surface
  diff go here>

### Fixed

- **`mod tests { use super::*; }` no longer binds every name of its file to
  the file's methods.** The glob named the file itself, and every bare name,
  field and method call of the file bound to each method of that name at the
  resolved-via-import tier. This gave the callee_overlap false edges `drop` ->
  `Drop::drop` (tokio broadcast.rs) and `heap_limit` ->
  `SearcherBuilder.heap_limit` (ripgrep). The guide's statements that a Rust
  call on a receiver never counts for callee_overlap, and that a call through a
  selector resolves to no edge, now hold for Rust.
```

## 5. Residuals (not fixed by this work, measured or derived)

- Lower-case path heads that bind a function of the same name (`json::parse` with a function named
  `json`): 21, 12 and 35 binding sites remain (measured, v4 set).
- Tokens after `::` in macro arguments (`tokio::spawn` in a macro): 17, 2 and 178 binding sites
  remain after R3 (derived from the macro-path-tail class). They resolve as bare names.
- Nested function items and block-level constants are no symbols (crates/facts/src/extract/
  rust_items.rs walks no function body), so a call of a nested `fn helper` still binds a top-level
  `helper` elsewhere. Not measured.
- Trait methods (`function_signature_item`, default bodies in `trait_item`) are no symbols.
- Use-tree leaves keep the role `reference`, not `import` as in Python. Their tier is right.
- A closure inside macro arguments (`assert_eq!(r.map_err(|error| error.kind), ..)`) is a token
  tree, so R1 does not see its parameters. Seen once in the class (c) sample; not counted.
- The prototype did not run R5 or the parameters of `function_signature_item` and
  `function_type`. P3 measures them.

## 6. Open questions for the owner

**Q1. Members: bind nothing (Python parity), or bind as name-only candidates?**
Recommended: bind nothing (R2 and R4 as specified). Reasons: the binding tiers are the same under
both options; the guide and health.rs already state that a selector call resolves to no edge; no
new role value, wire value or resolver branch is needed. The cost, measured: a Rust method's
`explain` fan-in falls to none (27 -> 0 for `Searcher.line_terminator`), and the file graph loses
the member names (`is_reference` in this repo: 156,790 with R1 alone, 98,864 with R1 to R4, -37%).
The other option is a new role `member`
that binds only methods and only at the name-only tiers; it keeps those candidates for Rust and,
later, Python. Take it as a separate, additive ticket if agents miss the candidates.

**Q2. Bind `self.f()`, `Self::f()` and `T::f()` to `T.f` by owner (R6)?**
Recommended: not in RSC; a separate ticket with its own surface diff and reading. Reasons: R6 adds
binding edges, so it needs a precision measurement that this ticket does not have; RSC is a
precision fix. The cost of leaving it out, measured: the 232, 280 and 244 owner-matched sites that
bind today only through the self-glob, and the 6 true callee_overlap rows of section 3.5.

**Q3. Apply R3 (no method for a bare name at the import and name-only tiers) to every language?**
Recommended: Rust only in RSC. The same-file tier already applies the rule to every language
(`in_module_scope`). Python, JavaScript and Go would follow the same rule, but C++, Java, Kotlin
and C# call a method by bare name inside a class (implicit `this`), so the rule is per language. A
later ticket can key it on a language-table value with a Python surface diff (the map ranking does
not read the resolver, so the retrieval tier cannot see it).

**Q4. The kind lists as data in rust.toml, or as consts in rust_scopes.rs?**
Recommended: data, as specified in D1, because the brief asks for it and because the generator
gives the lists a grammar check that consts do not have. The cost: an xtask change, which needs
`scripts/gate-selftest.sh`. Python's lists stay consts in python_scopes.rs; moving them is a
separate change.

**Q5. R5 (item and variant names are declarations): in RSC or later?**
Recommended: in RSC, P1. It is small (74, 23 and 71 binding sites, measured) and has the Python
precedent (`Role::Declaration` for a `def` or `class` name). It is the one rule that the prototype
did not run, so P3 must measure it before the CHANGELOG states its figures. A line that only
declares then leaves refs, so refs `total` can fall for such targets.

**Q6. The 3 new callee_overlap rows of the self tree.**
Recommended: one blind reader labels them in P3, with the CO1B packet format, before the release
claims a false-positive figure for Rust.

## 7. Sources

- Code at 81d5a7e: crates/facts/src/extract/identifier_refs.rs:16-37, 98-108;
  crates/facts/src/extract/python_scopes.rs (the Python model); crates/facts/src/extract/
  rust_items.rs:31-50, 147-157; crates/facts/src/extract/rust_uses.rs:301-306;
  crates/facts/src/declared.rs:58-71; crates/facts/src/values.rs:253-263;
  crates/graph/src/resolver.rs:117-144, 173-187, 233-251, 363-366; crates/graph/src/scopes.rs:
  67-74, 315-341, 455-489; crates/graph/src/rust_paths.rs:144-184; crates/graph/src/names.rs:
  199-216; crates/graph/src/files.rs:172-205; crates/graph/src/lookup.rs:163-175;
  crates/app/src/refs.rs:288-348; crates/app/src/refs/sites.rs:82-144;
  crates/app/src/capabilities.rs:200, 231; crates/render/src/capabilities.rs:30-31;
  crates/app/src/callees.rs:89-114; crates/app/src/health.rs:21-23, 130-150;
  crates/patchcheck/src/callers.rs:55-68; crates/syntax/languages/rust.toml.
- History: f7b0d89 (graph fidelity: the Rust `use` walk, R1 to R2a, LIFE, LANG) and b64d502 (its
  review fixes), read with `git show -s`.
- The vendored grammar pack (crates/syntax/vendor/grammars-1.14.3.tar.zst) holds parser.c,
  scanner.c and headers only; no query file. Upstream tree-sitter-rust at the vendored rev
  77a3747266f4d621d0757825e6b11edcbf991ca5 ships highlights.scm, injections.scm and tags.scm, and
  no locals.scm (GitHub tree, read with fetch_page). The grammar rules of D1 come from its
  grammar.js at that rev (read).
- nvim-treesitter queries/rust/locals.scm (read): a candidate design. It marks only direct
  identifier patterns (`let_declaration pattern: (identifier)`, `tuple_pattern`, `parameter`), has
  no `struct_pattern`, `ref_pattern` or `or_pattern` rule, no ordering for `let`, and no barrier
  for nested items. It cannot give the `let x = x;` answer or the nested-fn answer, so D1 does not
  use a query.
- rust-analyzer crates/hir-def/src/expr_store/scope.rs (read): `ExprScopes`, the `let` scope after
  the initializer, closure and match-arm scopes, `if let` in the then-branch, let chains.
- rustc-dev-guide, "Name resolution" (read): ribs, namespaces, a nested function does not see the
  outer locals.
- github/stack-graphs (repository README, read): GitHub no longer supports or updates it. Its
  `languages/` page (read) holds only the versioning README in the extracted text, so I did not
  verify which language definitions it ships. I found no Rust definition. Not verified: treat
  stack graphs as no ready design for Rust here.
- docs/analysis/benchmark-methodology.md (the release regression gate) and AGENTS.md at 81d5a7e.
- CO1B: the packets szri, u859, nxkq, vwfd; key/findings.jsonl; out/m4; the labels of co1b-r1 and
  co1b-r2 (read only).

## 8. Work products (W = /var/tmp/claude-rust-port/rsc)

- W/scripts/rsc-measure.patch, W/scripts/rsc_proto.rs, W/scripts/rsc_dump.rs: the prototype and
  the dump hook (measurement only).
- W/bin/measure: the measurement binary, COMMIT, SHA256SUMS.
- W/dump/{self,ripgrep,tokio}.jsonl: one row per site that binds in some variant, plus aggregates.
- W/out/count3.txt, co1b_compare.txt, co1b_labels.txt, locbench_maps.json,
  refs_preview_*.json: the measured outputs.
- W/scripts/count.py, sample.py, owner_check.py, co1b_compare.py, locbench_maps.py,
  refs_preview.py: the scripts.

## 9. Owner answers (2026-11-04)

The owner approved this specification and the recommended answer of each question of section 6.

- Q1. Members bind nothing: R2 and R4 as specified.
- Q2. R6 (the owner binding of `self.f()`, `Self::f()` and `T::f()`) is not in this work. A later
  ticket does it.
- Q3. R3 applies to Rust only.
- Q4. The kind lists are data in crates/syntax/languages/rust.toml.
- Q5. R5 is in RSC, phase P1.
- Q6. The reader of the 3 new callee_overlap rows belongs to phase P3.
