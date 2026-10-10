# Specification: Erosion, Clone, and Contract-Drift Checks

**Date**: 2026-10-09 | **Status**: revision 2, after two adversarial reviews (Codex gpt-6.1-sol and Gemini gemini-3.8-flash, both FAIL on revision 1); nothing built | **Research input**: `mcp-local/docs/research/2026-10-09_slopcodebench-code-erosion.md`

## Problem Statement

Coding agents pass tests while the code they extend gets worse with each change. SlopCodeBench (arXiv 2603.24755) measured this: complexity concentrated in a few functions rose in 77% of agent runs, and duplicated code grew 96% across checkpoints. Telling agents to write clean code lowered the starting level of the decay but did not slow its rate. The levers with evidence behind them give the agent a check it cannot argue with, applied at each change, with detailed feedback.

Our tooling has no such check. No gate measures complexity growth, duplicated blocks, or silent defaults on foreign data, and nothing compares a change against the code before it. In mcp-local, erosion rose from 0.251 to 0.423 over ten months and fell only when a manual audit split one function.

This spec adds three before/after checks to agentless-mcp and one per-edit hook that runs them:

| Check | Question it answers | Gate use |
|---|---|---|
| `complexity_delta` | Did this change make a complex function more complex, push a function over the threshold, or add a new function over it? | Per-edit hook and review |
| `contract_drift` | Did this change add a silent default or a swallowed error in a module that parses foreign data? | Per-edit hook and review |
| `clone_delta` | Did this change add a block that duplicates code elsewhere in the repository? | Review first, per-edit hook after the cache stores block hashes |

It also adds a complexity section to `health`, so agents and audits can see where complexity is concentrated and how it trends.

## Locked Decisions (user, 2026-10-09)

1. **Placement**: inside agentless-mcp, not a new tool. agentless already has the grammars, qualified names, before/after checks, and an owned, audited dependency set.
2. **Hook mode**: advisory first, with a switch to blocking after a trial.
3. **Languages**: all 12 tier-1 grammars in the first release (bash, c, cpp, go, java, javascript, lua, python, ruby, rust, tsx, typescript).
4. **Fidelity**: Python numbers must match scb-check v0.1.3, the version the benchmark harness pins.
5. **Contract drift**: same phase as erosion, not a later spec.

## Research Summary

### Key Findings

- **Decay is the norm under iteration, and prompts do not slow it.** Erosion rose in 77% of agent trajectories. The anti-slop prompt lowered the start level by 34-58% (erosion) but left the rate at about 1.3 pp per checkpoint. (Source: SlopCodeBench, arXiv 2603.24755, RQ2 and RQ4)
- **The mechanism is local patching.** Agents add branches to an existing function instead of extracting. One `main()` grew from CC 13 to CC 92 over 8 checkpoints with no helper extracted. (Source: SlopCodeBench, Appendix B.2)
- **Erosion predicts the cost of later changes, not their correctness.** Correctness of later changes is predicted by drift in input validation and error handling, for example silent `dict.get(k, default)` fallbacks (OR 1.83, p = 0.011), while static complexity was not significant (p >= 0.78). (Source: SlopCodeBench Appendix B.3; CodeThread, arXiv 2606.21804)
- **Detailed feedback in the loop extends how long agents survive change.** Detailed test feedback gave 6-12x more turns than minimal feedback. (Source: StaminaBench, arXiv 2606.19613)
- **A metric gate alone invites gaming.** A SAST-only gate held its own metric at 0% while latent degradation rose. Locally, audit reports record helpers extracted only "to stay under the PLR0912/PLR0915 gate". (Source: SCAFFOLD-CEGIS, arXiv 2603.08520; `mcp-local/.audit/execution/D-RUNTIME-report.md:38`)
- **Complexity accumulates in small increments, and the fix is zero tolerance on each change.** After each change, the code should have the structure it would have had if it had been designed with that change in mind. (Source: Ousterhout, *A Philosophy of Software Design*, 2.4 and 16.1)
- **The counter-technique is to put new behavior in a new function.** (Source: Feathers, *Working Effectively with Legacy Code*, Sprout Method and Sprout Class, p. 268-269)
- **Count decision points, not lines.** Start at 1 and add 1 for each `if`, `while`, `repeat`, `for`, `and`, `or`, and each `case`. (Source: McConnell, *Code Complete*, Table 19-2)
- **Analyzer trust depends on a low effective false-positive rate.** An issue is an effective false positive when the developer takes no positive action after seeing it. Google tracks this per analyzer, disables analyzers that do not improve, and fixes confusing messages, because a misread message counts the same as a wrong one. (Source: Winters et al., *Software Engineering at Google*, "Focus on Developer Happiness" and "Integrated Feedback Channels")

### Measured Baselines

| Measurement | Value | Command |
|---|---|---|
| mcp-local `src/` erosion, scb-check | 0.350 (HEAD edb10a6) | `uvx scb-check==0.1.3 check --report --include-all src` |
| mcp-local `src/` verbosity, scb-check | 0.191 | same |
| scb-check runtime | 2.2 s for `src/`, 0.26 s for one file | `/usr/bin/time` |
| agentless CLI startup | 0.11 s | `agentless-mcp --help`, 3 runs |
| agentless `lint`, all seven checks, one edit | 0.79-0.86 s on mcp-local (463 files, warm cache) | `agentless-mcp lint --candidates <file> --repo . --json` |

Most of the lint time builds whole-repository facts (resolver, manifests, every file's text) that the new checks do not need.

### Approach Decision

Add a pure core module that computes per-function metrics from a parse tree, driven by per-language tables. Add three checks that compare whole-file text before and after a change. They take typed text pairs, not `RepoFacts`, so `lint` can run them without the repository scan the seven existing checks need. Add a stdlib-only PreToolUse hook to `contrib/hooks/` that turns a Write or Edit call into a text pair, runs the checks, and reports or blocks.

Why this shape:
- **Lint already holds the pattern.** `cycle_delta` reports only cycles a patch adds (`core/patchlint.py:2050-2099`). `syntax_delta` passes an edit only if it adds no parse errors (`core/normalize.py`). The comment gate blocks only findings the file did not already carry (`~/.claude/hooks/comment_rules.py:216-224`). The new checks follow the same before/after rule.
- **Lint stays advisory.** "Everything here is advisory. No finding blocks anything" (`core/patchlint.py:67-70`). Blocking is the hook's decision, from its own mode setting. Lint stays CLI-only by design (`application/lint_service.py:9-12`).
- **The hook stays outside the package.** Like the two existing gate hooks, it imports only the standard library (`contrib/hooks/agentless_gate_check.py:38-40`). Unlike them, it calls the agentless CLI as a subprocess. The existing gate hooks do not call the CLI, so that part has no precedent in this repository.
- **`health` gets the trend view.** It is already the whole-repository structure report on the MCP read surface, so agents can ask where complexity is concentrated without a write-side tool.

### Trade-offs Accepted

- **Two complexity definitions on the machine.** ruff's C901 stays in each repo's lint config. agentless counts complexity the scb-check way. They answer different questions, and only agentless gates on change.
- **Per-language tables live in the new module, not in `LanguageConfig`.** The extractor comment says node-type tables belong together (`core/extractor.py:111-122`), but python, rust, c, and cpp have no `LanguageConfig` row, and another session is editing `extractor.py` now. A test pins that the complexity tables cover exactly the tier-1 set, so the two stay in step.
- **Python parity on the benchmark's terms.** We copy scb-check's choices even where another tool would differ: Python `case` arms are not counted, comprehensions are, and a nested function's branches count toward its parent as well as itself. We also copy its quirks where they change numbers (D1, "Parity quirks").
- **Parity is defined on Python 3.12 and later.** scb-check requires 3.12. On 3.10 and 3.11 the tokenizer counts a multi-line f-string as fewer lines (Codex probe: SLOC 2 on 3.10, 4 on 3.13), so numbers there can differ in either direction. agentless still supports 3.10, and the capabilities report says which rule is in force.
- **Growth that adds no decision point is not detected.** A unit at CC 11 that grows from 100 to 400 SLOC doubles its mass, and D2 emits nothing. v1 gates on decision points only. `health` hotspots (D6) show mass, so the review step can still see it.
- **Clone detection joins the per-edit hook later.** A repository-wide block index needs either a full parse per edit or a cache. The cache change waits for the other session's single-parse work.

## Design

### D1. Metric model (`core/complexity.py`, new, pure)

**Unit.** A unit is a named callable. Anonymous callables (lambdas, closures, anonymous arrow functions) are not units. Their branches count toward the enclosing unit, which is what scb-check does for Python lambdas.

**Per-unit record.**

| Field | Meaning |
|---|---|
| `qualname` | `Class.method` or `outer.inner`, with no collision ordinal. Ordinals follow source order (`core/symbols.py:285`, `disambiguate`), so they shift when a same-name unit is inserted above another. D2 matches units without them. |
| `path`, `start_byte`, `end_byte` | The unit's identity in one text. Byte spans separate several callables on one line, which line spans cannot. |
| `start_line`, `end_line` | 1-based span, for messages |
| `body_key` | sha256 of the unit's token stream with comments removed and the unit's own name left out. D2 uses it to pair unchanged and renamed units. |
| `cc` | 1 + decision points in the unit's whole subtree, nested units included |
| `sloc` | Source lines of code inside the span (see SLOC rules) |
| `mass` | `cc * sqrt(sloc)` |

**File and repository aggregates.** `erosion = sum(mass for cc > T) / sum(mass)`, with `T = complexity_threshold` (default 10, strict `>`). An empty denominator gives 0.

**Unit discovery.** Python follows scb-check exactly: units are `function_definition` nodes reached from module children, `class_definition`, `decorated_definition`, and `block`. A function inside a module-level `if` or `try` is not a unit, because scb-check's walker does not descend into those. Other languages use the unit table below. A unit's name comes from the node's `name` field, or from the binding named in the table for a callable that has no name of its own. A callable with neither is not a unit, and its branches count toward the enclosing unit.

| Language | Unit node types | Name from a binding (callable has no name field) |
|---|---|---|
| javascript | `function_declaration`, `generator_function_declaration`, `method_definition` | `arrow_function`, `function_expression`, `generator_function` bound by a `variable_declarator` name, a `field_definition` property, or a `pair` key |
| typescript, tsx | `function_declaration`, `generator_function_declaration` (typescript), `method_definition` | as javascript, with `public_field_definition` in place of `field_definition` |
| go | `function_declaration`, `method_declaration` | none (`func_literal` is not a unit) |
| java | `method_declaration`, `constructor_declaration`, `compact_constructor_declaration` | none (`lambda_expression` is not a unit) |
| c | `function_definition` | none |
| cpp | `function_definition` | none (`lambda_expression` is not a unit) |
| rust | `function_item` | none (`closure_expression` is not a unit) |
| ruby | `method`, `singleton_method` | none (`lambda`, `block`, `do_block` are not units) |
| lua | `function_declaration` | `function_definition` bound by a `variable_declaration` or `assignment_statement` with one target |
| bash | `function_definition` | none |

Every name in this table was checked as a named kind in the installed grammars (language pack 1.14.3, `Language.id_for_node_kind`, 2026-10-09).

C and C++ `function_definition` has no `name` field. Its name comes from the declarator chain: follow `declarator` fields, and the first named child of a `parenthesized_declarator`, down to the identifier-like node. A C++ `qualified_identifier` such as `A::B::go` gives the qualname `A.B.go`, as the extractor spells it. A C++ `lambda_expression` is never a unit.

**Qualified names and stable ids.** The qualname is the enclosing unit's or class's qualname plus the unit's name. The class-like scopes are: python `class_definition`; javascript `class_declaration`, `class`; typescript and tsx `class_declaration`, `abstract_class_declaration`, `class`; java `class_declaration`, `interface_declaration`, `enum_declaration`, `record_declaration`; cpp `class_specifier`, `struct_specifier`, `union_specifier`, `namespace_definition`; rust `impl_item` (named by the first `type_identifier` of its `type`), `trait_item`, `mod_item`; ruby `class`, `module`. A Go method's qualname also carries its receiver type (`Store.Find`), as the extractor's Go ids do. c, lua, and bash have no scopes. When an agentless symbol starts at the same place, the unit takes that symbol's stable id. `ASTSymbol` carries line numbers only, with no start byte (`core/symbols.py:102-103`), so Phase 4 either adds a byte field or matches on start line and name. A unit with no matching symbol (for example an arrow function bound in an object `pair`, which the extractor does not report today) is a metric-only unit. It has no stable id, and every surface that shows it says so.

**Decision points per language.** One table row per tier-1 language. Each row lists decision node types and a boolean-operator rule. Node names below come from the installed grammars (language pack 1.14.3) and were verified as named kinds. The grammar test also checks full counts on fixtures, because a correct node name with a wrong predicate (a default arm, an operator in an unexpected parent) passes a name check.

| Language | Branches and loops | Case arms counted | Boolean operators | Other |
|---|---|---|---|---|
| python | `if_statement`, `elif_clause`, `for_statement`, `while_statement`, `except_clause`, `conditional_expression` | none (parity) | `boolean_operator` | `assert_statement`, `list_comprehension`, `set_comprehension`, `dictionary_comprehension`, `generator_expression`, `if_clause` |
| javascript, typescript, tsx | `if_statement`, `for_statement`, `for_in_statement`, `while_statement`, `do_statement`, `catch_clause`, `ternary_expression` | `switch_case` | `binary_expression` with operator `&&`, `\|\|`, `??` | none |
| go | `if_statement`, `for_statement` | `expression_case`, `type_case`, `communication_case` | `binary_expression` with `&&`, `\|\|` | none |
| java | `if_statement`, `for_statement`, `enhanced_for_statement`, `while_statement`, `do_statement`, `catch_clause`, `ternary_expression` | `switch_label` with a named child (not `default`), `guard` | `binary_expression` with `&&`, `\|\|` | `assert_statement` |
| c | `if_statement`, `for_statement`, `while_statement`, `do_statement`, `conditional_expression` | `case_statement` without a `default` token | `binary_expression` with `&&`, `\|\|` | none |
| cpp | as c, plus `for_range_loop`, `catch_clause` | as c | `binary_expression` with `&&`, `\|\|`, `and`, `or` | `lambda_expression` is never a unit |
| rust | `if_expression`, `for_expression`, `while_expression` (`loop_expression` has no condition, and the `if` that breaks out of it already counts) | `match_arm`, except an unguarded arm whose `match_pattern` is only `_`; plus 1 for a guard (`if` inside `match_pattern`) | `binary_expression` with `&&`, `\|\|`; each `&&` child of `let_chain` | none |
| ruby | `if`, `elsif`, `unless`, `if_modifier`, `unless_modifier`, `while`, `until`, `for`, `while_modifier`, `until_modifier`, `rescue`, `rescue_modifier`, `conditional` | `when`, `in_clause` except an unguarded arm whose pattern is only `_`, plus 1 for `if_guard`/`unless_guard` | `binary` with `&&`, `\|\|`, `and`, `or` | none |
| lua | `if_statement`, `elseif_statement`, `for_statement`, `while_statement`, `repeat_statement` | none (no switch) | `binary_expression` with an anonymous `and`/`or` child (no operator field) | none |
| bash | `if_statement`, `elif_clause`, `for_statement`, `c_style_for_statement`, `while_statement` (also covers `until`), `ternary_expression` | `case_item`, except an item whose only pattern is `*` | anonymous `&&`/`\|\|` children of `list`; `binary_expression` with `&&`, `\|\|` in any parent (`test_command` and the `(( ))` `compound_statement`); `-a` or `-o` only as the `operator` field of a `binary_expression` (the unary `[ -a file ]` is a file test) | none |

`else` clauses are never counted, in every language, matching both McConnell and scb-check. Default arms are never counted. JS/TS `default:` is a separate `switch_default` node, so `switch_case` already excludes it. A JS/TS `for...of` loop parses as `for_in_statement`, so the row already covers it.

**Non-Python case arms follow McConnell** (+1 per case, default excluded). This diverges from Python's parity rule on purpose: Python must match the benchmark, and no benchmark exists for the others. The user confirmed this choice (Q1).

**SLOC rules.**
- **Python**: copy scb-check. A line counts when a `tokenize` token other than COMMENT, NL, NEWLINE, INDENT, DEDENT, or ENDMARKER starts on it. Then remove every line of a plain-string statement (docstring) that owns its lines. In the pack grammar a docstring is a bare `string` child of a `block` or `module` (the `expression_statement` wrapper scb-check relies on is hidden in ABI 14), with no `b` or `f` prefix.
- **Other languages**: a line counts when a leaf node starts on it that is outside every comment and has text other than whitespace. Named and anonymous leaves both count, as Python counts every token: a named-leaf rule drops `return;`, `break;`, `} else {`, `end`, and `fi`. String content spanning lines counts its first line only when the grammar gives it as one leaf. Bash gives one `string_content` leaf per line, so each line of a Bash multi-line string counts.
- **Version note**: Python 3.12 tokenizes f-strings into parts (PEP 701), so a multi-line f-string counts more lines on 3.12+ than on 3.10/3.11. scb-check requires 3.12+. Parity is defined on 3.12+ (Trade-offs Accepted).

**Parity quirks.** Codex probes against scb-check v0.1.3 found these behaviors. Each changes numbers, so the Python path copies each one, and each gets an exact fixture in D10.
- SLOC splits the source with `str.splitlines()`, not the tokenizer's `readline()`. Embedded Unicode line separators (U+2028 and similar) therefore move token line numbers (`scb_check/analysis/loc.py:37`).
- The docstring ownership test slices a Unicode `str` with tree-sitter byte columns (`loc.py:82`). On `"éééééééé"; x=1` scb-check gives SLOC 2, where a character-correct test gives 3.
- Clone normalization drops plain-string statements without the ownership test (`clones.py:161`). The SLOC predicate and the clone predicate are therefore two functions, not one.
- A `TYPE_CHECKING` block is not a clone candidate, but traversal continues into it, so functions inside it are candidates (`clones.py:128`).
- Clone normalization also drops a one-element string tuple statement (`"a",`). In the pack grammar that is a `tuple_expression` with one named `string`, so the Phase 4 normalizer must drop that shape too. It does not affect SLOC, because the comma always fails the ownership test (ERO-1 probe).
- The clone hash is md5 truncated to 12 hex digits (`clones.py:178-183`). Parity compares clone lines, not hash values, so agentless keeps sha256. A collision in the 48-bit truncated hash is the one case where the two could group differently.

**Parse and tokenize failures.** A file that does not parse, or that `tokenize` rejects (`tokenize.TokenError`, `SyntaxError`, including `IndentationError`), gives one `not_checked` row for that file. The other files in the same run are still checked. These exceptions are caught by name in the complexity code. They are not added to `DEGRADED_ERRORS` (`core/patchlint.py:214`).

### D2. `complexity_delta` check

Inputs: text pairs (D5.2), one per changed file.

**Matching.** Old and new units are paired in three passes. Each pass sees only the units the earlier passes left unpaired.
1. **Body pass.** Units with the same `body_key` pair up, in source order within each key. This pairs unchanged units, units renamed without other change, and units moved within the file. A pair from this pass has equal cc, so it never gives a finding.
2. **Name pass.** A qualname held by exactly one unpaired unit on each side pairs those two units.
3. **Collision pass.** For a qualname held by more than one unpaired unit on either side, sort the old cc values and the new cc values from high to low, and pair them by rank. This compares the group as a multiset, so inserting or reordering same-name units does not change the result. Example: swapping unchanged units with cc 11 and cc 20 gives the pairs (20, 20) and (11, 11), and no finding.

A new unit that no pass pairs is "new". In every input form, the body pass runs across all the files of one run: one candidate, one hook call, or one review (`--since`, D5.4). It pairs units within each file first, then pairs the units that every file left, across files, so a unit moved between files pairs with its old copy, and a copy beside its unchanged original is new in the other file. The name and collision passes then run within each file, because a qualname carries no path. Candidates stay separate runs, because they are alternative patches.

| Case | Finding |
|---|---|
| Paired unit, old `cc > T`, new `cc > old cc` | grew while over the threshold |
| Paired unit, old `cc <= T`, new `cc > T` | crossed the threshold |
| New unit with `cc > T` | new unit over the threshold |
| `cc` fell, or unit removed | none |

- **Severity**: `warning`.
- **Location**: the unit's line in the new text.
- **Message**: states what changed, why it matters, and how to fix it, because a misread message counts as a false positive (Software Engineering at Google). Example: `register_search_tools: complexity 16 -> 17, 194 -> 201 lines. This function is already above the threshold (10), and this change adds a branch to it. Put the new branch in a new named function and call it, or refactor this function first in its own change.`
- **Evidence**: `cc old -> new; sloc old -> new; threshold T`.
- **Not a finding**: a renamed unit at or below the threshold, and any unit the body pass paired.
- **Known gap, per-edit hook**: the hook sees one file per call. A complex unit moved to another file arrives as a Write of the new file, and the hook reports it as "new unit over the threshold". The message says "new, or moved from another file". The log records the match pass for every finding (D8), so the D9 report can measure how often this happens. Review mode does not have this gap.

### D3. `contract_drift` check

Scope: only files that match `boundary_globs` from `.agentless-mcp.json`. With no globs configured, the check emits one `not_checked` row ("no boundary modules configured") per candidate that edits at least one tier-1 file, and nothing else. A candidate that edits no tier-1 file gets no row. A global default would mostly flag correct internal defaults: 102 such patterns exist in mcp-local today, and many read internal data. In JSON, this row carries `detail` `{"reason": "no_boundary_globs"}`, and the c and bash row carries `{"reason": "no_rules_for_language", "language": "<name>"}`, so a caller matches the reason and never the message.

Rules: data, not code. One JSON file, `src/agentless_mcp/prompts/contract_rules.json`, because each rule's message is a caller-facing string. Each rule has an `id`, `languages` (a list, so one query serves javascript, typescript, and tsx), a tree-sitter `query`, a `message`, and a `severity`. The prompts package loads it once at import and validates it eagerly: unknown keys fail, and every field is typed. Each rule language is checked at core import. The queries compile on first use per language, so the unit test that compiles every query against its grammar, with the predicate allowlist and the capture names, is the gate for rule wiring. New rules are a data change plus a fixture. This is how the rule set grows over time.

**Rules are syntactic only.** A tree-sitter query sees syntax, not types or data flow. v1 has no rule that needs to know a value's type (for example that a Go result is an `error`), or whether a later line checks a result. Each rule id names the syntax it matches, not the intent behind it.

**Glob semantics.** A glob matches the repo-relative POSIX path. `*` matches within one path segment, and `**/` matches zero or more whole segments, so `**/adapters/**` matches both `adapters/x.py` and `src/pkg/adapters/x.py`. The repository has no glob matcher today (searched `core/` and `util/`), so `core/contracts.py` adds one with its own tests. It does not use `fnmatch`, whose `*` crosses `/`.

**Delta key.** Each hit becomes the key `(rule_id, unit, capture)`:
- `unit` is the qualname of the enclosing unit, taken from the D2 pairing so that a renamed unit keeps its old identity. A hit outside every unit uses `<module>`.
- `capture` is the matched text with its comment nodes cut out and each run of whitespace collapsed to one space. Identifiers, receivers, and keys are kept, so `data.get("items", [])` and `resp.get("items", [])` are different keys. A comment edit inside a hit does not change its key.

Old and new keys are compared as multisets, like the comment gate's `new_findings` (`comment_rules.py:216-224`). In every input form, the multisets span all the checked files of one run (D2), and each unit's identity in the key is the old unit the D2 pairing gave it, with that unit's file. A hit that moves with its function to another boundary file is therefore not new. A file outside `boundary_globs` counts on neither side, so a function moved into a boundary module brings its hits in as new. Only new keys become findings, at `warning` severity. Because the key holds the unit, moving `data.get("items", [])` from an internal helper into a response parser in the same file is a new hit. A variable rename in a boundary module re-reports its hits. That cost is accepted, because those hits sit in a module that parses foreign data.

Initial rules (v1):

| Language | Rule ids |
|---|---|
| python | `get-default-empty` (`.get(k, [])`, `{}`, `""`, `0`), `or-empty-default` (`x or {}`, `x or []`), `getattr-default`, `except-return-default` (`except` returning `None`, `[]`, `{}`, `False`, `0`), `except-pass` |
| javascript, typescript, tsx | `nullish-empty-default` (`?? []`, `?? {}`), `or-empty-default` (`\|\| []`, `\|\| {}`), `catch-return-default`, `catch-empty` |
| go | `err-check-returns-nil` (a `return` whose last value is `nil`, inside an `if` whose condition is `err != nil`) |
| rust | `unwrap-or-default`, `unwrap-or-empty`, `ok-statement` (`.ok()` as a whole expression statement) |
| java | `catch-empty`, `catch-return-null`, `get-or-default-empty` |
| ruby | `rescue-nil` (modifier and block), `fetch-default-empty`, `or-empty-default` |
| lua | `or-empty-table` (`x or {}`), `pcall-statement` (`pcall(...)` as a whole statement, so its status is never read) |
| cpp | `catch-all-empty` (`catch (...) {}`) |
| c, bash | none in v1. C has no syntactic marker that separates a silent default from a deliberate one. Bash's `\|\| true` is usually deliberate. The check emits a `not_checked` row for these languages. Both reviews recommend this, and the user confirmed it (Q2). |

Dropped from revision 1, because a query cannot tell the cases apart: Go `error-discarded` (`value, _ := f()` parses the same whether `_` drops an error or a count), Rust `let-underscore-result` (`let _ = f()` holds no type), and Lua `pcall-unchecked` (it needs data flow to a later line).

### D4. `clone_delta` check

- **Normalization (Type-2)**: a new function in `core/normalize.py`, beside `normalized_stream`. It renames identifiers to `$VAR1`, `$VAR2`, and so on, by order of first appearance within the block, maps literals to type tokens (`$STR`, `$INT`, `$FLOAT`, `$BOOL`, `$NONE`, from a per-language literal table), and drops comments and docstrings. It hashes the result with sha256. Python copies scb-check's `_normalize_ast`, adjusted for the hidden `expression_statement`.
- **Candidates**: per-language block types with 3 or more SLOC. Python uses scb-check's set: `function_definition`, `if_statement`, `for_statement`, `while_statement`, `with_statement`, `try_statement`, `match_statement`, skipping `if TYPE_CHECKING:` blocks. Other languages use their function, loop, branch, and try node types from D1.
- **Delta**: build two complete repository indexes, before and after, with every changed file of the run applied together. Keep every candidate, including singletons, so a block whose only twin is new still has a baseline. Pair moved instances first: a hash's instances in deleted or changed old text pair with its instances in new text by count. A finding is a new-text instance of a hash whose repository-wide count rose (`after - before > 0`), reported at that instance's path and line. Moving one of two existing instances to another file does not raise the count, so it gives no finding. Severity is `advisory`, like `near_duplicates`.
- **Overlap with `near_duplicates`**: `near_duplicates` keeps its scope (functions the patch introduces, exact token stream) and its output. `clone_delta` skips a function-definition candidate that `near_duplicates` already reported at the same path and line in the same run. Merging the two is a later decision.
- **Repository index**: computed on demand (one parse per file, Type-2 keys for every candidate) until Phase 5 stores the keys in the cache.

### D5. Lint plumbing

The revision 1 plan, "lazy facts" inside `RepoFacts`, conflicts with the current design. `RepoFacts` has four required fields, and its contract forbids building repository views in core (`core/patchlint.py:374`). `LintService._facts` scans the repository before any check runs (`application/lint_service.py:100-114`). `lint_patch` parses fragments and reads dependency warnings outside the per-check guards (`core/patchlint.py:694-716`). Revision 2 leaves all of that unchanged and gives the new checks their own input.

1. **Check registry and `--only CHECK[,CHECK...]`.** A registry in `application/lint_service.py` lists each check and the input it needs: `patch` (the seven existing checks, which need `RepoFacts` and parsed fragments), `pairs` (`complexity_delta`, `contract_drift`), or `pairs+clones` (`clone_delta`). An unknown name is a usage error (exit 2). Without the flag, all checks run, as today. When no selected check needs `patch`, the service does not call `_facts` or `lint_patch`. Fact gathering stays in application.
2. **Text pairs.** The new checks take a typed value, `TextPair(path, old, new)`, from core. `old` is `None` when the file did not exist, and `new` is `None` when the file is deleted. An empty string is an existing empty file. Missing, empty, unreadable, and oversized are four different states, and each unreadable or oversized side gives a `not_checked` row. Both sides are read as bytes and decoded as UTF-8 with no newline translation, so CRLF files keep their line endings. Each side is capped at `DEFAULT_MAX_FILE_BYTES` (1,000,000 bytes, `util/fslimits.py:35`). Pairs come from three sources:
   - **Candidates** (`--candidates`, existing). For pair checks, the service reads only the edited files with the bounded reader, runs `unidiff.orientation` against those texts, applies the edits, and forms pairs. An edit with an empty search side on a missing path is a creation (`old` is `None`). The candidate path keeps its current rule for an empty search on an existing empty file: orientation and the applier reject it (`core/unidiff.py:224`, `core/patches.py:623`).
   - **Hook form** (`--path REL --after FILE`, new). The service reads the old text from the working tree at `REL` and the new text from `FILE`. No SEARCH/REPLACE document is built, and orientation does not run. The hook writes only the proposed new text, so a 600 KB source no longer turns into a 1.4 MB candidate that the 1,000,000-byte candidate reader rejects (Codex probe).
   - **Review mode** (`--since REV`, item 4).
3. **Per-file degradation.** The new checks catch degraded input per file. One file that fails to parse gives a `not_checked` row for that file, and the other files are still checked. The existing `_guarded` wrapper returns one gap row for the whole check and drops its other results (`core/patchlint.py:722`), so the new checks do not use it.
4. **Review mode, `--since REV`** (Phase 4). A separate entry point, `LintService.lint_revision`, CLI-only like the rest of lint (`application/lint_service.py:9-12`). It accepts only pair checks. `--since` with a `patch` check is a usage error (exit 2), because those checks compare against the current tree and would see a mix of two states. Steps:
   - Resolve `REV` with `git rev-parse --verify --end-of-options REV^{commit}`. A bad revision is a usage error.
   - Changed files: `git diff --name-status -M -z REV` (the working tree against `REV`, staged and unstaged changes both), plus untracked files from `git ls-files --others --exclude-standard -z`.
   - Renamed: `old` is the blob at the old path, `new` is the working-tree file at the new path. Deleted: `new` is `None`. Added or untracked: `old` is `None`.
   - Binary files and submodule entries each give one `not_checked` row. Files with a suffix outside the tier-1 set are out of scope and give no row.
   - As built (ERO-5a): the list comes from `git diff --raw -z -M --relative REV --`, because only `--raw` carries the modes that name a submodule (`160000`) or a symlink (`120000`), and `--relative` keeps paths relative to a `--repo` below the git top level. A file is binary when a NUL byte falls in its first 8,000 bytes, git's own rule, applied to the bytes already read. A symlink side counts as absent, because its blob is a target path. A path with an unmerged or unknown status gives one `not_checked` row. A rename across languages becomes a deletion and an addition. A git call that cannot run, times out, or fails gives a `not_checked` row with exit 0, and so does a listing over 8,000,000 bytes, which checks nothing because a partial list would report moved units as new. Only a revision that git rejects is a usage error, through the `RevisionUnresolved` error (exit 2). No change since `REV` gives one `not_checked` row. The report has one candidate, and its id is `REV` as typed.
   - Blob reads: one bounded `git cat-file blob REV:PATH` per file through `run_bounded`. `run_bounded` opens no stdin pipe (`core/gitinfo.py:356-372`), so `git cat-file --batch` would need a change to its contract, and v1 does not make that change. Stage 5 of the other session already builds `cat-file blob` arguments (`core/githistory.py:80`, `blob_arguments`). Reuse that function.
   - The caller computes the merge-base, for example `--since "$(git merge-base HEAD main)"`.
5. **Speed.** Target: a hook-form run at or under 0.25 s on mcp-local (derived target, measured in the Phase 2 gate). Codex measured CLI import plus `build_parser()` at 0.107 s median over 5 fresh processes. That leaves about 0.14 s (derived) for configuration, git, reads, grammar loading, and parsing.
6. **Check-count text.** Update every place that says "seven checks" or "the other six checks" (`core/patchlint.py:8-10` and `:211`, `src/agentless_mcp/docs/agent-guide.md:1106-1114` and `:1136`).
7. **`observed` on each lint JSON candidate** (ERO-5a). It describes the new text, not the change, so a later record for the same path shows whether a unit got simpler or a hit went away (D8, D9). Each list is present only when its check measured at least one file of the candidate, and `observed` is absent when no list is. Every row is an object, and every `unit` is the qualname in the new text, or `<module>`:
   - `units_over_threshold`: `{path, unit, line, cc, sloc}` for each unit in the new text with `cc > T`, unchanged units included;
   - `units_changed`: `{path, unit, line, match, cc_old, cc_new, sloc_old, sloc_new}` for each match that is not a body-pass match, with `match` one of `name`, `collision`, `new`, and `null` old values for a new unit;
   - `contract_keys`: `{path, rule, unit, capture}` for each hit in the new text of each file `contract_drift` checked, with duplicates kept.
   A `contract_drift` finding's `detail.unit` keeps its D3 meaning, the paired old identity. The text output does not change.
8. **The check list in `capabilities`** (ERO-5a). Text and JSON (`lint_checks`) list each registered check with its input kind (`patch` or `pairs`), so the hook reads the registry and never parses an error message.

### D6. `health` complexity section (Phase 4)

- **Summary line**: units counted, units over the threshold, erosion, and clone share.
- **Clone share**: the number of distinct `(path, line)` entries that are SLOC lines inside any instance of a group with 2 or more instances, divided by total SLOC, in the same scope. Using the union of SLOC lines keeps the share at or under 1: overlapping and nested candidates count a line once, and docstring lines inside a span do not count.
- **New section `hotspots`**: the top units by mass, with `stable_id` (or "metric-only unit", D1), path, line, cc, sloc, and mass. `HealthSection` rows today must be `HealthSymbol`, which has degree fields and no complexity fields (`application/render.py:1183`). This section adds a new row view, `HealthComplexityRow`, and keeps the existing bounds and `limit`.
- **Exclusions**: test and fixture paths, using the same `is_test_path` and `is_fixture_path` rules health uses today. This also matches the research finding that test files dilute erosion.
- **Computation**: on demand until Phase 5, then read from the cache.
- **Time budget**: `deadline.checkpoint()` raises `CallStopped` and ends the whole call (`util/deadline.py:79`). On a large repository, the new walk could therefore break the `health` output that works today. The walk keeps its own budget: it stops after 10 s (derived) or half of the call's remaining allowance, whichever is less. It then reports "complexity: N of M files walked before the time budget" as a coverage line, and the other sections return as before. The walk still calls `checkpoint()` per file, so a client that leaves still stops the call.
- **Wire text**: update the `orient` tool description and `orient_limit` in `prompts/tool_descriptions.json` and `prompts/parameter_descriptions.json`, and the agent guide's health section. The tool description is caller-facing API, so it states what the section answers and what it does not.

### D7. Project configuration (`.agentless-mcp.json`)

| Key | Type and bounds | Default | Read by |
|---|---|---|---|
| `complexity_threshold` | int, 1-100 | 10 | `complexity_delta`, `health` |
| `boundary_globs` | list of at most 64 glob strings, each at most 256 characters, repo-relative, semantics in D3 | empty | `contract_drift` |
| `erosion_gate` | object mapping a check name (`complexity_delta`, `contract_drift`, `clone_delta`) to `"off"`, `"advisory"`, or `"block"` | unset (every check uses the hook's environment default) | the hook only |

`erosion_gate` is per check because D9 qualifies each check on its own. A repository can block `complexity_delta` while `contract_drift` stays advisory.

All keys go through `KNOWN_KEYS`, are validated the existing way (a bad value is dropped with a warning, never clamped), and appear in `capabilities` under effective config.

### D8. Per-edit hook (`contrib/hooks/agentless_erosion_gate.py`, new)

- **Event**: PreToolUse, matcher `Write|Edit`. Claude Code 2.1.288 offers no MultiEdit tool (S0 Q7). `NotebookEdit` is the one other file-editing tool, and the hook does not cover it.
- **Imports**: the standard library only, like the two existing gate hooks.
- **Overall deadline**: 10 s from hook start, on a monotonic clock. It covers root discovery, reads, configuration, lint, and logging, not lint alone. Each lint run gets the time that remains, less 0.5 s for logging. Claude Code sends SIGTERM to a hook that passes its settings timeout (S0 Q6). The hook's SIGTERM handler deletes the temp file, logs `degraded` with reason `terminated`, and exits 0. The handler is the second limit, after the 10 s deadline.
- **Steps**:
  1. Read `AGENTLESS_EROSION_GATE`. The value `off` is a machine-wide switch that wins over every repository key, so the hook exits 0 before any other work and writes no log record. The values `advisory` and `block` are defaults that a repository key overrides (step 4). Any other value is dropped with a warning, and `advisory` applies.
  2. Read the payload from stdin, capped at 4 MiB. A larger or malformed payload is `degraded`. The payload must carry `session_id`, `transcript_path`, `tool_use_id`, `tool_name`, and `tool_input.file_path` as strings. S0 found all of them in 13 of 13 payloads (Q1). A missing `replace_all` is `false`, because the model's wire input can omit it.
  3. Resolve the target with `os.path.realpath`. Find the root with `git -C <nearest existing ancestor of the unresolved target> rev-parse --show-toplevel` (2 s timeout), so a Write into a new directory still finds its repository. Skip with exit 0, logged, when the suffix is not a tier-1 extension, there is no repository, the resolved target is outside the root (for example through a symlink), or the agentless binary is missing. The binary is `AGENTLESS_EROSION_BIN` when set, else `agentless-mcp` on `PATH`. The suffix set is copied in the hook. A drift test pins it to the `SUPPORTED_EXTENSIONS` suffixes that `erosion.pair_language` admits, the same way `test_gate_hooks.py:475-529` pins operation names.
  4. Resolve the mode per check: the repository's `erosion_gate` entry for that check (bounded read of `.agentless-mcp.json`), then `AGENTLESS_EROSION_GATE`, then `advisory`. An invalid entry is dropped with a warning, as `projectconfig` drops one. When every check is `off`, exit 0.
  5. Read the old text as bytes, capped at 1,000,000 bytes, and decode it as UTF-8 with no newline translation. A missing file has no old text. An oversized or non-UTF-8 file is `degraded`.
  6. Compute the proposed text. Write gives `content` verbatim, so a Write over a CRLF file leaves LF bytes (S0 Q2). Edit follows Claude Code's line-ending rule, which matched the written bytes for 3 of 3 observed files (S0 Q2). Count CRLF and bare LF in the old text, normalize CRLF to LF, and apply `replace(old_string, new_string)` on the normalized text, to every occurrence with `replace_all` and to one without it. Then convert every line ending to CRLF when CRLF was the majority. A tie gets LF, and the log records `eol: "tie"`. Line endings change neither cc, sloc, nor contract hits. The hook skips an Edit that Claude Code fails: an empty `old_string`, an `old_string` equal to `new_string`, an `old_string` absent from the normalized text, and a non-unique `old_string` without `replace_all`.
  7. Write the proposed text to a file from `tempfile.mkstemp(prefix="agentless-erosion-")` (mode 0600, unique per call, so parallel calls do not collide). SIGTERM is blocked while the file is created and its path is recorded. Close the file before the subprocess starts. Delete it in a `finally` block, which also runs after a timeout, an exception, or SIGTERM.
  8. Run `agentless-mcp lint --repo <root> --path <rel> --after <tmp> --json --only <checks not off>` (D5.2 hook form). The hook's known checks are `complexity_delta` and `contract_drift`. `clone_delta` joins the list in Phase 5, after its own trial (D9). The fast path is one lint call. When lint exits 2, the hook runs `agentless-mcp capabilities --repo <root> --json` once, under the same deadline rule, and parses `lint_checks` (D5.8) strictly as a list of `{name, input}` objects. It then runs lint once more with the requested checks that the list names with input `pairs`, and it logs each dropped check. The call is `degraded` with the reason when `capabilities` fails, its output does not parse, it has no `lint_checks`, or no requested check remains. When every requested check remains, the refusal has another cause, so the hook does not retry and logs lint's usage error. A binary whose `capabilities` has no `lint_checks` and whose lint usage line has no `--path` is `degraded` with reason `agentless-mcp lacks the hook form`. The hook does not read check names from an error message.
  9. Parse the response against the lint JSON shape (`application/render.py:1330`): a `candidates` list of one candidate whose `findings` carry `check`, `severity`, `message`, `path`, `line`, and `evidence`. A finding's `detail` and the candidate's `observed` are absent or objects, and the hook logs them unchanged. Any other shape is `degraded`.
  10. Classify. `warning` and `advisory` findings go to their check's mode. A `contract_drift` row of severity `not_checked` whose `detail.reason` is `no_boundary_globs` or `no_rules_for_language` is a designed off state for that file, not a gap, and the record lists the check in `off_by_config`. Every other `not_checked` row is a gap. The outcome is `blocked` when a finding comes from a `block` check, else `advisory` when any finding exists, else `degraded` when a gap exists, else `clean`. The record's `gaps` list keeps every gap, so the outcome is never `clean` while a gap exists.
- **Output**:
  - **Advisory**: exit 0 with `{"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": "<findings>"}}` on stdout and no `permissionDecision`. This is the documented form (Claude Code hooks reference, "PreToolUse decision control"). S0 confirmed that the text reaches the model as a system reminder (Q3), and that the call still goes through the normal permission flow (Q4). In the S0 control, `permissionDecision: "allow"` approved a write in a session that could approve nothing, so the hook never emits one.
  - **Block**: when any finding comes from a check in `block` mode, exit 2 with the findings on stderr: blocking findings first, then advisory findings under their own label. Exit 2 blocks the call, and stderr reaches the model as the tool result, after a prefix that names the hook command (S0 Q5). The hook prints nothing on stdout with exit 2, because S0 did not test whether Claude Code ignores stdout JSON on exit 2.
  - **Clean, degraded, skipped, and off**: exit 0 with no output.
- **Message format**: at most 15 findings, each `path:line: message`. Then one line naming the mode and how to change it.
- **Failure policy**: any internal error exits 0 and is logged. "A gate that breaks an unrelated session is worse than a gate that misses one call" (`agentless_gate_check.py:29-33`). The whole hook body runs under a catch-all, unlike `comment_gate.py`, which has none. A timed-out hook gives the model no text, so a timeout looks clean to the agent (S0 Q6). Only the log and the transcript's `hook_cancelled` attachment show it.
- **Log**: when `AGENTLESS_EROSION_LOG` is set, append one JSONL record per hook call, findings or not. The log is the input to D9. Each record holds:
  - session id, `transcript_path`, `tool_use_id`, tool name, and path. The transcript does not hold the call at hook time (S0 Q1, 13 of 13), so the hook never reads it. The D9 report joins on `tool_use_id` after the session.
  - sha256 of the old text and of the proposed text. An Edit on a mixed-ending file rewrites every line ending (S0 Q2), so the proposed text can differ from the old text on lines that the edit did not touch.
  - the mode per check, the checks that lint ran, the checks dropped, and the outcome: `clean`, `advisory`, `blocked`, `degraded`, or `skipped`, with the reason and the gaps.
  - per finding: check, severity, mode, path, line, message, evidence, and lint's `detail`. For `complexity_delta`, `detail` holds the unit qualname, the match pass (body, name, collision, or new), cc and sloc old and new, and the threshold. For `contract_drift`, it holds the D3 key as `rule`, `unit`, and `capture`.
  - observed state of the proposed text: the lint JSON `observed` object (D5.7), copied as lint returns it, or null when lint sends none. `units_over_threshold` rows are `{path, unit, line, cc, sloc}`, `units_changed` rows are `{path, unit, line, match, cc_old, cc_new, sloc_old, sloc_new}`, and `contract_keys` rows are `{path, rule, unit, capture}`. A list is absent when its check measured no file. Every `unit` is the qualname in the proposed text. A later record for the same path shows whether the unit got simpler or the hit went away. The report compares contract hits on `(path, rule, capture)`, so a rename does not read as a fix.
  - latency.
- **Settings**: add an entry to `contrib/hooks/settings-example.json` with `"timeout": 15`. A timed-out PreToolUse hook does not block the call (S0 Q6), which matches the failure policy. Do not set `onFailure: block`.
- **Parallel hooks**: Claude Code runs matching PreToolUse hooks in parallel (S0 Q6). The erosion hook runs beside `comment_gate.py`, and neither can rely on the other's result or order.

### D9. Advisory-to-block flip criterion

Revision 1 gated the flip on the effective false-positive rate alone. Both reviews rejected that. In advisory mode an agent can ignore a correct finding, and the effective rate counts that as a false positive. The rate then measures agent behavior and message clarity together, and it may never fall under the bar. The revision 1 log also could not tell a correction from an unrelated edit. Revision 2 measures two signals per check and gates on the one the agent cannot move.

**Report.** `contrib/hooks/erosion_gate_report.py` reads the D8 log and classifies each finding:
- **Not applied**: the transcript at `transcript_path` shows the tool call failed or was rejected (by the user, or by another hook). Excluded from both signals.
- **Unobserved**: no later record for the same session and path. Reported as a count. Not a false positive.
- **Acted on**: a later record in the same session shows the unit's cc at or under its old value, the branch moved into a new unit, or the hit key gone.
- **Not acted on**: observed, with none of the above.

**Labeled precision (the gate).** At the end of a trial window, the report draws a seeded random sample of 30 applied findings per check (all of them if fewer). The user labels each one: correct (the finding names a real change worth acting on) or wrong (a counting error, a mispaired unit, or an intended pattern). Precision is correct over labeled.

**Effective false-positive rate (a diagnostic).** Not acted on over observed, as Software Engineering at Google defines it. High precision with a high effective rate points at the message, not the rule. That is the case where Google fixes the message.

**Flip, per check.** Set that check's `erosion_gate` entry to `block` when all three conditions hold for that check alone:
1. at least 50 applied findings of that check in the trial;
2. labeled precision at or above 90%;
3. no change to the check's rules or message since its trial started. A change restarts that check's trial.

The 90% bar is our own derived target. The corpus passage states only that Google deploys analyzers "with low false-positive rates", with no number. A check that stays below the bar gets its rule or message fixed, or it stays advisory, as Google does with analyzers that do not improve. `clone_delta` starts its own trial when it joins the hook in Phase 5, and it cannot block before it qualifies.

**Outcome.** Precision says the findings are right. It does not say the gate slows erosion. The research report labels this gate design derived and untested (research report section 4.3). The trial therefore also records `health` erosion for each trial repository at the start and the end, next to the same span of time before the trial.

### D10. Python parity harness

- **Reference generator**: `scripts/scb_parity_goldens.py` runs at development time only. It calls scb-check v0.1.3's `extract_functions()` and `detect_clones()` through `uvx --from scb-check==0.1.3 python`, and writes per-unit `(name, start_line, cc, sloc)`, clone lines, and erosion per fixture to `tests/characterization/goldens/complexity/*.json`. scb-check never becomes a runtime or test dependency.
- **Fixture**: `tests/characterization/fixtures/complexity_py/`. It covers comprehensions, boolean chains, `match` with guards, nested functions, lambdas, decorated and async definitions, functions inside module-level `if` and `try` (which scb-check skips), docstrings, multi-line strings and f-strings, Type-1 and Type-2 clones, and `TYPE_CHECKING` blocks. It also covers each D1 parity quirk: a U+2028 inside a string, a docstring followed by code on the same line with non-ASCII text, a docstring with a trailing comment inside a clone, and functions nested in a `TYPE_CHECKING` block.
- **Pass rule**: the unit inventories must be equal. Every `(name, start_line)` scb-check reports, agentless reports, and the reverse. For each unit, cc and sloc match exactly. The erosion difference is under 1e-9, and clone lines match exactly.
- **Runtime**: parity tests run on Python 3.12 and later, the versions scb-check supports. In the CI matrix that is 3.13. On 3.10 the parity module is skipped, and one separate test pins the documented f-string difference, so a change in it is seen.
- **Live check**: a manual verification step compares agentless erosion on one snapshot of mcp-local `src/` with scb-check's erosion on the same snapshot. The Phase 1 gate measures through `core.complexity` directly, and Phase 4 repeats the check through `health`. It is not a test, because it depends on another repository.
- **Other languages**: hand-counted goldens per fixture under `fixtures/complexity_<lang>/`, reviewed against the D1 table and McConnell's rule.

## Implementation Plan

### Principles to Follow

- **Zero tolerance on each change, not a ceiling.** Gate on growth, never on an absolute limit. (Source: Ousterhout, 2.4 and 16.1)
- **Before/after only.** A finding the file already carried is never reported again. The one known exception is a complex unit moved between files under the per-edit hook (D2, "Known gap"). (Source: comment gate `new_findings`; `cycle_delta`)
- **Fail open, log everything.** (Source: `agentless_gate_check.py:29-33`; vault hooks `INSTALL.md:147-149`)
- **Messages say what, why, and how to fix.** (Source: Software Engineering at Google, "Integrated Feedback Channels")
- **Rules are data with eager validation.** (Source: the prompts loader pattern, `prompts/loader.py`)
- **Measure before flipping.** (Source: Software Engineering at Google, "Focus on Developer Happiness")

### Patterns to Apply

- **Before/after multiset delta** (`comment_rules.new_findings`): `contract_drift` keys and `clone_delta` group counts.
- **Old and new whole-file text** (`_cycle_delta` with `apply_edits`): `complexity_delta`.
- **Degraded input becomes a row** (`patchlint.DEGRADED_ERRORS`): each new check turns degraded input into a `not_checked` row, per file rather than per check (D5.3).
- **Stdlib hook with drift tests** (`contrib/hooks/*`, `tests/unit/test_gate_hooks.py`): the new hook.
- **Golden characterization tests** (`tests/characterization/test_lint_goldens.py`, regenerated deliberately): a lint golden for the new checks.

### Anti-patterns to Avoid

- **Absolute complexity ceilings**: they produce extraction done only to pass the gate (F15 in the research report).
- **Inline suppression markers**, such as scb-check's `# scbc boundary:`. Scope by path instead. Never silence a check in source.
- **Global contract rules**: they flag correct internal defaults. Scope to boundary globs.
- **Prompt-only enforcement**: it lowers the start level but not the rate (SlopCodeBench RQ4).
- **Keeping parse trees in facts or cache**: forbidden by `core/cache.py:14-20`.
- **Auto-approving from an advisory hook**: a PreToolUse `permissionDecision: "allow"` must not bypass the user's permission prompt.

## Phases and Gates

The other session's stages (`docs/research/2026-10-09_evaluation-findings-solutions.md` section 3.10) touch `extractor.py`, `resolve.py`, `githistory.py`, `sandbox.py`, the cache, and the graph goldens. This plan starts with new files and the lint path, and moves extraction and cache changes after its stage 6.

Revision 1 said Phases 0-3 touch only new files and the lint path. That is not true. Phase 2 also touches `capability_service.py`, `projectconfig.py`, and `CHANGELOG.md`. Phase 4 touches `render.py`, `graph_service.py`, the prompt JSON files, and `normalize.py`. Each of those files may also be dirty in the other session. Before each phase, check `git status` and `git diff` on that phase's files, and agree on the order with the other session for any file it has changed.

Phase 5 is larger than revision 1 said. After stage 6, `extract_facts` parses once and returns only symbols, imports, and references. The tree is dropped (`core/extractor.py:1984-2000`, called from `core/cache.py:1464`). Caching metrics therefore needs either a second parse or a change to the extractor interface so that the same parse also returns unit metrics and block hashes. Phase 5 makes that interface change, so `extractor.py` is in its file list. The extractor must never import `core/complexity.py`, which already depends on the extractor (directly, and through `normalize`). The caller that owns the single parse sits above both modules and passes the tree to each. Cached metrics and hashes carry an algorithm version, separate from `SCHEMA_VERSION`. A change to the D1 tables or the normalizer bumps it, and rows with an old version are recomputed.

| Phase | Work | Depends on | Gate |
|---|---|---|---|
| 0 | **S0 hook spike**: log real PreToolUse payloads for Write and Edit. Prove that the documented advisory form reaches the model and does not auto-approve. Record whether the payload carries `tool_use_id`, and how Edit treats a CRLF file. **S1**: write `scripts/scb_parity_goldens.py` and generate the Python goldens. | none | S0: a written note with the payloads and the chosen output form. S1: goldens committed. |
| 1 | `core/complexity.py`: tables (D1), unit walk, SLOC rules, mass, erosion. Grammar-name validation test. Python parity tests. Hand goldens for the 11 other languages. | Phase 0 S1 | Parity tests pass on 3.13. Every table node name is a named kind in its grammar, and the full-count fixtures pass (default arms, guards, Bash arithmetic and `-a`/`-o`). Live mcp-local erosion matches 0.350 within 0.005. |
| 2 | Lint plumbing (D5.1-D5.3, D5.5, D5.6), `complexity_delta` (D2), `contract_drift` with rule data (D3), config keys (D7), lint golden, agent guide and CHANGELOG. | Phase 1 | All unit and golden tests pass. Hook-form lint at or under 0.25 s on mcp-local (measured). `pre-commit run --all-files` green. |
| 3 | Hook (D8) with tests, `erosion_gate_report.py` (D9), README install section, settings example. Deploy advisory on this machine. Start the trial log and record the starting `health` erosion. | Phases 0 and 2 | Hook tests pass. A pipe test blocks in `block` mode and reports in `advisory` mode. One live session shows the advisory text reaching the model. |
| 4 | Review mode `--since` (D5.4), `clone_delta` (D4) with an on-demand index, `health` complexity section (D6). | Phase 2; reuse the stage 5 blob reader if present | Tests and goldens pass. `health` on mcp-local within the MCP call limit (30 s). |
| 5 | Extractor interface change so one parse also returns unit metrics and block hashes. Cache them (`SCHEMA_VERSION` 15 to 16) with an algorithm version. `clone_delta` joins the hook in advisory mode and starts its own trial. | Other session's stage 6 (single parse) | Cache tests pass. A D1 table change invalidates cached metrics. The hook with `clone_delta` stays at or under 0.5 s on mcp-local. |
| 6 | Skill integration in the vault (source of truth), then copy down: architect-review Growth step runs `lint --since <merge-base> --only complexity_delta,clone_delta,contract_drift --json`; pr-review adds it as evidence; codebase-review stores `health` complexity in its baseline; the pm-mode executor brief explains the hook. | Phase 4 | Each skill change reviewed. |
| 7 | Flip each check to `block`, per repository, when it meets D9. | Phase 3 trial data | The D9 criterion is met for that check, with the labeled sample and the start and end erosion recorded. |

Deferred: cognitive complexity, tier-2 languages, merging `near_duplicates` into `clone_delta`, and contract rules for c and bash.

## Task Checklist

**Progress pin, 2026-10-09.** Branch `feat/erosion-checks` in the worktree `agentless-mcp-erosion`, from `main` `d7f0c52`. Nothing committed. Q2 and Q3 take the reviewers' recommendation (not_checked for c and bash, empty `boundary_globs`) unless the user redirects. Ticket ERO-1 (Phase 0 S1 and Phase 1) is landed in the worktree, unvalidated, and uncommitted: `pytest` 3694 passed and 10 skipped (baseline 3585 and 10), pre-commit green, and live erosion on mcp-local `f17af96` equal to scb-check's 0.3500945549598066 over 1770 units. S0 (hook spike) not started: it needs a logging hook in a live session, which needs the user's go-ahead.

- [ ] S0: log real hook payloads, confirm the advisory form live, record `tool_use_id` presence and CRLF Edit behavior in this file
- [ ] S1: write `scripts/scb_parity_goldens.py`, generate and commit Python goldens
- [ ] Write `core/complexity.py` with the D1 tables, unit walk, SLOC, mass, erosion
- [ ] Add the grammar-name validation test for every complexity, unit, and contract table entry, plus full-count fixtures
- [ ] Add Python parity tests and hand goldens for the other 11 languages
- [ ] Add the check registry and `--only` to `lint`; add `TextPair`, the candidate pair path, and the `--path/--after` hook form
- [ ] Add `complexity_delta` with the three-pass matching and message text per D2
- [ ] Add `contract_drift`, `prompts/contract_rules.json`, the eager loader, the glob matcher, and rule fixtures (positive and negative)
- [ ] Add `complexity_threshold`, `boundary_globs`, and `erosion_gate` to project config and `capabilities`
- [ ] Update check-count text, the agent guide lint section, `test_guide.py` heading counts, and the CHANGELOG
- [ ] Write `contrib/hooks/agentless_erosion_gate.py`, its tests, and `erosion_gate_report.py`
- [ ] Update `contrib/hooks/settings-example.json` and the README hook section
- [ ] Deploy the hook in advisory mode on this machine and start the log
- [ ] Add `lint --since` as `lint_revision`, reusing stage 5's `blob_arguments`
- [ ] Add the Type-2 normalizer and `clone_delta`
- [ ] Add the `health` complexity section and update the wire text
- [ ] After stage 6: extractor interface change, cache metrics and block hashes with an algorithm version, bump `SCHEMA_VERSION`
- [ ] Update the review skills in the vault and copy them down
- [ ] Flip each check to `block`, per repository, when D9 is met for it

## Files to Modify

New:
- `src/agentless_mcp/core/complexity.py`: unit walk, per-language tables, SLOC, mass, erosion
- `src/agentless_mcp/core/contracts.py`: rule index, query compiler, glob matcher, and query matcher
- `src/agentless_mcp/core/erosion.py`: `TextPair` and the three pair checks (`complexity_delta`, `contract_drift`, `clone_delta`), returning `patchlint.Finding` rows so rendering stays shared
- `src/agentless_mcp/prompts/contract_rules.json`: rule data
- `src/agentless_mcp/prompts/contract_rules.py`: the eager rule-file parser and its typed record
- `contrib/hooks/agentless_erosion_gate.py`: per-edit hook
- `contrib/hooks/erosion_gate_report.py`: log analysis for D9
- `scripts/scb_parity_goldens.py`: development-time reference generator
- `tests/unit/test_complexity.py`, `tests/unit/test_contracts.py`, `tests/unit/test_erosion_gate_hook.py`
- `tests/characterization/fixtures/complexity_*/`, `tests/characterization/goldens/complexity/`

Modified:
- `src/agentless_mcp/core/patchlint.py`: check-count text and module docstring only; `RepoFacts` and `lint_patch` do not change
- `src/agentless_mcp/application/lint_service.py`: check registry, `--only`, the candidate pair path, the `--path/--after` hook form, `lint_revision` for `--since`
- `src/agentless_mcp/adapters/cli/main.py`: `--only`, `--path`, `--after`, and `--since` flags
- `src/agentless_mcp/core/normalize.py`: `node_stream` for `body_key` (Phase 1), Type-2 normalizer (Phase 4)
- `src/agentless_mcp/util/errors.py`: the `UnmeasurableSource` leaf for a file `core/complexity.py` cannot measure
- `src/agentless_mcp/prompts/messages.json` and `prompts/__init__.py`: the three `UnmeasurableSource` reasons
- `pyproject.toml`: deptry `extend_exclude` gains `scripts`, because the parity generator imports `scb_check` under uvx
- `.github/workflows/ci.yml`: the skip ceiling, for the parity module's skip on Python 3.10
- `src/agentless_mcp/core/projectconfig.py`: three keys
- `src/agentless_mcp/application/capability_service.py`: effective config rows
- `src/agentless_mcp/core/githistory.py` or `core/gitinfo.py`: reuse stage 5's `blob_arguments` for single bounded blob reads; no change to `run_bounded`
- `src/agentless_mcp/util/deadline.py`: a read-only query for the call's remaining allowance (D6 time budget)
- `src/agentless_mcp/application/graph_service.py` and `application/render.py`: health section
- `src/agentless_mcp/prompts/tool_descriptions.json` and `parameter_descriptions.json`: health wire text
- `src/agentless_mcp/core/extractor.py`: Phase 5 interface change so one parse also returns unit metrics and block hashes
- `src/agentless_mcp/core/cache.py`: Phase 5 columns, algorithm version, and `SCHEMA_VERSION`
- No packaging change: the `uv_build` backend ships files under the module directory, as it does for `prompts/*.json` today. The installed-wheel test (Verification Steps) proves that `contract_rules.json` ships.
- `src/agentless_mcp/docs/agent-guide.md`, `README.md`, `CHANGELOG.md`
- `contrib/hooks/settings-example.json`
- `tests/characterization/test_lint_goldens.py` and its goldens, `tests/unit/test_guide.py`

Outside this repository (Phase 6, vault first):
- `claude-backup/claude/skills/{architect-review,pr-review,codebase-review,pm-mode}/`
- `claude-backup/claude/hooks/INSTALL.md` (hook install note)

## Verification Steps

- [ ] `uv run pytest` passes on Python 3.10 and 3.13 (the CI matrix); parity tests run on 3.13
- [ ] Python parity goldens match exactly (D10)
- [ ] Every node name in the complexity, unit, and contract tables is a named kind in the installed grammar, and the full-count fixtures pass
- [ ] An installed-wheel test loads `contract_rules.json` from the built wheel, compiles every query, and runs one rule
- [ ] `agentless-mcp health` erosion on mcp-local `src/` is within 0.005 of scb-check's 0.350
- [ ] `agentless-mcp lint --path <rel> --after <file> --only complexity_delta,contract_drift` runs at or under 0.25 s on mcp-local (`/usr/bin/time`, 5 runs, median)
- [ ] `uv run pre-commit run --all-files` is green, including `lint-imports`
- [ ] Hook pipe tests: advisory mode prints the frozen JSON form and exits 0, block mode exits 2, a malformed payload exits 0, a lint timeout exits 0 and logs `degraded`, the temp file is gone after every path
- [ ] Report tests: a fixture log and transcript give the expected not-applied, unobserved, acted-on, and not-acted-on counts, and the seeded sample is stable
- [ ] A live session confirms advisory text reaches the model and no permission prompt is skipped

Edge cases to test:
- [ ] A unit renamed in the same edit, a unit moved to another file (hook: reported as new or moved; `--since`: paired), two methods with the same name in different classes
- [ ] Same-name units inserted above, swapped, or reordered (the collision pass gives no finding)
- [ ] A contract hit moved from an internal helper into a parser in the same file (a new key)
- [ ] Moving one of two clone instances to another file (no `clone_delta` finding)
- [ ] An Edit with `replace_all`, a Write that creates a file, a Write that empties a file
- [ ] A file with a syntax error before or after the edit, and a Python file that `tokenize` rejects (that file degrades to `not_checked`)
- [ ] A run with one unparsable file and one file with a real regression (the regression is still reported)
- [ ] A language with no contract rules (c, bash), and a repository with no `boundary_globs`
- [ ] Unicode identifiers, CRLF line endings, a file over the 1,000,000-byte cap, an existing empty file, a Write into a directory that does not exist yet, a target that is a symlink out of the repository
- [ ] `--since`: a renamed file, a deleted file, an untracked file, a binary file, a bad revision

## Risks

| Risk | Mitigation |
|---|---|
| Another session is editing agentless now (26 dirty files on 2026-10-09) | Check `git status` and `git diff` on each phase's files before the phase starts, and agree the order for shared files (Phases and Gates). Extraction and cache work waits for stage 6. |
| The pack's Python grammar differs from scb-check's (ABI 14 vs 15; hidden `expression_statement`) | Docstring detection adapted in D1. Exact parity tests in D10 show any drift. |
| A language pack upgrade renames node types | The grammar-name validation test fails on upgrade. |
| Agents split functions just to pass the gate | Gate on growth only. Review skills judge whether an extraction names a real concept (Phase 6). The log shows patterns. |
| The advisory form auto-approves edits | The documented form has no `permissionDecision`. Spike S0 confirms it live before any hook ships. |
| Hook latency is noticeable on every edit | The hook form skips the repository scan. A 10 s overall deadline inside the hook, and a 15 s hook timeout that fails open. |
| The gate passes its precision bar but does not slow erosion | D9 records `health` erosion at the start and end of each trial. A gate with high precision and no erosion effect is a finding to report, not a reason to flip. |
| Agents ignore advisory findings | The D9 effective rate shows it. The flip gates on labeled precision, so ignored findings do not hold back a correct check. |
| `contract_drift` false positives | Boundary globs, advisory first, and the D9 criterion per check. |

## Open Questions

1. **Q1, case arms**: RESOLVED 2026-10-09. Non-Python languages count each non-default case arm (McConnell). Python keeps scb-check's rule for parity.
2. **Q2, c and bash contract rules**: ship v1 with a `not_checked` row for these two languages, or define narrow rules now (for example Bash `|| true` on a command whose output a later line parses)? **Both reviews recommend `not_checked` for v1.** The Bash example needs data flow to a later line, which a query-only rule set cannot see (D3, "Rules are syntactic only"). In C, a returned `NULL` or status code is ordinary control flow. Narrow rules can follow once each has positive and negative fixtures. RESOLVED 2026-10-09: the user accepted the recommendation.
3. **Q3, boundary default**: keep `boundary_globs` empty by default (the check is off until configured), or default to a conventional set such as `**/adapters/**` and `**/infrastructure/**`? **Both reviews recommend empty.** A directory name does not show that a module parses foreign data. The research report separates foreign-response defaults from correct internal defaults inside the same repositories (research report section 3.2). The `not_checked` row ("no boundary modules configured") keeps the off state visible. RESOLVED 2026-10-09: the user accepted the recommendation.
4. **Q4, Strict scoping**: RESOLVED 2026-10-09. The user approved amending `~/.claude/CLAUDE.md` so that a refactor of the function being edited is in scope. The hook's block message relies on this.

## Review Record

Revision 1 went to two independent read-only reviews on 2026-10-09: Codex `gpt-6.1-sol` (17 findings) and Gemini `gemini-3.8-flash` (8 findings, 3 missing items). Both returned FAIL. Each finding was checked against the code or with a parse probe before revision 2 used it.

| Topic | Raised by | Result | Where revision 2 changes |
|---|---|---|---|
| Flip criterion cannot separate a wrong finding from an ignored one; log cannot see corrections | both | confirmed | D8 log, D9 |
| Collision ordinals shift; rename and move identity | both | confirmed (`symbols.py:261`, `:285`) | D1 record, D2 matching |
| `--since` would be refused by `unidiff.orientation` | both | confirmed (`lint_service.py:321`) | D5.4 |
| Temp file lifecycle and concurrency | both | confirmed | D8 step 7 |
| Lazy facts conflict with `RepoFacts` and `lint_patch` | Codex | confirmed (`patchlint.py:374`, `:694-716`; `lint_service.py:100-114`) | D5.1, D5.2 |
| Whole-file candidate exceeds the 1,000,000-byte reader | Codex | confirmed (`fslimits.py:35`) | D5.2 hook form |
| CRLF translation and existing empty files | Codex | confirmed | D5.2, D8 steps 5-6 |
| Contract rules that need types or data flow | Codex | confirmed | D3 rules |
| Contract key loses the unit | Codex | confirmed (derived counterexample) | D3 key |
| Clone delta per file, singletons dropped | Codex | confirmed (derived) | D4 delta |
| Bash `(( ))` and `-a`/`-o`; Bash `*` and Rust `_` default arms | Codex | confirmed by probe | D1 rows |
| Rust `loop_expression` counted | Gemini | confirmed | D1 rust row |
| Python parity quirks (splitlines, byte columns, clone docstrings, `TYPE_CHECKING`) | Codex | accepted from Codex probes; D10 fixtures will confirm | D1 parity quirks, D10 |
| Unit table missing; metric-only units | Codex | confirmed (all names probed) | D1 unit table |
| Blocking a check that never qualified | Codex | confirmed | D7 `erosion_gate`, D9 |
| Hook deadline, root, symlink, response parsing | Codex | confirmed | D8 |
| Clone share definition; `HealthSection` row type | Codex | confirmed (`render.py:1183`) | D6 |
| `health` time budget ends the whole call | Gemini | confirmed, worse than stated (`deadline.py:79` raises) | D6 |
| Coordination understated; stage 6 drops the tree | Codex | confirmed (`extractor.py:1984-2000`) | Phases and Gates, Phase 5 |
| Gate hooks do not call the CLI; guide path | Codex | confirmed | Approach Decision, D5.6 |
| JS/TS miss `for...of` and count `default:` | Gemini | refuted by probe: `for_in_statement`, `switch_default` | D1 note |
| Bash misses `until_statement` | Gemini | refuted by probe: `until` is `while_statement` | D1 bash row |
| sha256 breaks clone parity | Gemini | refuted: parity compares lines, not hashes | D1 parity quirks |
| Guide line numbers moved | Gemini | refuted: table still at 1107-1114 | none |

## Side Findings (out of scope, for the owner of each file)

- `core/extractor.py`: `_ECMASCRIPT_FUNCTION_VALUES` lists `"function"`, which is not a named kind in the javascript, typescript, or tsx grammars. `_ECMASCRIPT_IDENTIFIERS` lists `"type_identifier"`, which the javascript grammar lacks.
- `core/extractor.py`: `BODY_BLOCK_NODE_TYPES` includes `function_body`, which no tier-1 grammar has. It also includes `block`, which in Ruby is a brace closure, not a statement block.
- `~/.claude/hooks/comment_gate.py` has no catch-all, so an unexpected exception exits 1, not 0. The vault `INSTALL.md:147-149` says every gate fails open.
- `src/agentless_mcp/docs/agent-guide.md:1131-1137` says `undeclared_imports` reports `not_checked` on Python 3.10. `core/patchlint.py:22-24` and `:525` show 3.10 now falls back to `tomli`.
