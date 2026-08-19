# Cross-Block Synthesis (Phase 6)

Repo: `src/agentless_mcp/` at HEAD `bf7b21b`. Input: 24 block reviews, 358 findings
(5C / 65H / 135M / 153L), plus the cross-block flag list in `progress.md`.

This document names the patterns, not the bugs. A finding that appears once is a
bug and lives in its block file; a finding that appears in two or more blocks is a
**convention**, and fixing it one instance at a time is how it comes back. Every
pattern below cites the specific finding ids that establish it and, where a claim
was not already measured by a block reviewer, the verification run for this pass.

Verification performed in this pass (not inherited from block reviews):

- `grep -rn "Field(" src/agentless_mcp/adapters/mcp/server.py` -> **one** hit
  (`server.py:86`, a `description=` only). No `ge`/`le`/`gt` anywhere on the wire.
- `grep -rn "Truncation(" src/` -> **two** hits, `server.py:226` and `main.py:642`,
  both the map view. Six other limited listings pass nothing.
- `grep -rn "^from agentless_mcp" src/agentless_mcp/adapters/cli/main.py` -> 13
  `core` modules imported directly; `adapters/mcp/server.py` -> 6.
- `grep -rn "read_bounded(" src/` -> 10 call sites, three of which feed a write
  path (`patch_service.py:403`, `lint_service.py:110`, `lint_service.py:178`).
- `grep -rn "def load_candidates" src/` -> two definitions.
- `ls -d .github` -> **does not exist**. No CI of any kind.
- `pyproject.toml:262-274` -> one `layers` contract (`adapters -> application ->
  core -> prompts -> util`) and one `independence` contract naming only
  `adapters.cli` / `adapters.mcp`. No `forbidden` contract. `core` is one opaque
  member. `bootstrap` is outside the layer list by explicit decision
  (`pyproject.toml:250-253`).

---

## Part 1 — [SYSTEMIC]

Ranked by blast radius: how many blocks carry the smell x how far a wrong answer
travels before anything notices.

### S1 — Silent truncation: the service slices, the renderer recounts, and only one of eight listings announces the cut

**Blocks:** B02, B05, B16, B17, B18, B19, B22, B23.
**Files:** `application/render.py:738,776,783`, `application/symbol_service.py:190,206,213,247`,
`application/graph_service.py:182,209,422,452`, `application/envelope.py:53,151-158`,
`adapters/mcp/server.py:226`, `adapters/cli/main.py:642,1341-1352`.

The package's stated purpose is bounded views that say what they left out. The
mechanism exists — `envelope.Truncation` (`envelope.py:53`), documented for
"matches past a limit" — and is wired to exactly one view. Measured this pass:
`Truncation(` appears twice in `src/`, both times for `repo_map`.

Six listings slice and stay silent:

- `find_referencing_symbols`: 42 of 52 sites dropped at `limit=10`, header asserts
  "10 references", no marker (**B18-H1**, reproduced). `RefsResult` carries the
  honest `total`; `render_ref_groups` recomputes the header from the truncated
  tuple instead (**B05-H1**, reproduced at 7 sites / `limit=3`).
- `find_symbol`: same shape, `total` reaches JSON and nothing else (**B18-M1**, **B05-H1**).
- `explain_symbol`'s import sections: the total is never even *computed* —
  `graph_service.py:452` slices `rows[:limit]` and discards `len(rows)`, so
  `Explanation.as_dict()` has no field that could carry it. 30 importers render as
  20 (**B19-H1**, **B05-M1**, both reproduced).
- `analyze_structure operation=cycles`: the description says the operation "takes
  nothing" while `server.py:394` reads `request.limit` and `graph_service.py:182`
  cuts at 20 — so the renderer's honest `... N more cycles` line tells the agent
  something was dropped and the description has just told it there is no parameter
  to raise (**B02-M3**).
- `expand_symbols`: the unresolved flood is charged against no budget, so at 192
  ids the JSON envelope drops 14 of 40 *seated cards* — whole symbols, which the
  fair-split comment identifies as the failure it exists to prevent (**B18-H3**, measured).
- `MapResult.skipped` is collected by `core/refs.py`, carried into `as_dict()`, and
  rendered by no text renderer — so over MCP, which returns text only, files that
  were too large or whose grammar was cold are simply absent (**B17-M4**).

Two second-order effects make this worse than N missing lines. First, **text and
JSON diverge systematically in the same direction**: `as_dict()` carries
`total`/`limit`, the text form carries neither, and every MCP tool returns text
(**B18** cross-block, **B05** cross-block). `_Answer`'s docstring
(`main.py:1322-1327`) claims the two forms "cannot diverge by accident"; `_emit`
drops `answer.truncation` on the `--json` branch and `wrap_json` has no parameter
to receive it (**B16-M3**, **B23-M3**). Second, the `omitted` property is
copy-pasted into four value objects (`render.py:222,281,420,469`) while the two
listings that need it most have no wrapper at all (**B05-H1**).

**Fix direction.** Make the count structurally inseparable from the rows.
`SharedCallerListing` is the existing model and its docstring already argues the
case: a listing type carrying `rows`/`total`/`limit` with one shared `omitted`,
passed whole to the renderer, so no adapter can drop it. Then add `truncation` to
`wrap_json` and pass `answer.truncation` on both branches of `_emit`. Do not fix
this by threading a second `total` argument into six renderer signatures — that is
the shape that produced the bug.

### S2 — The MCP adapter validates nothing the CLI validates, and the services trust "the adapter checks"

**Blocks:** B11, B12, B17, B18, B19, B22, B23 (seven independent sightings).
**Files:** `adapters/mcp/server.py:512-513,540,553,557-559,568,583,597,616-618,648`;
`adapters/cli/main.py:1383,1389-1395`.

Verified this pass: `pydantic.Field` is imported and used once in the entire MCP
adapter, to attach a *description*. Not one numeric parameter on the wire carries
`ge` or `le`. Every `int`, `float` and `int | None` — `depth`, `max_entries`,
`limit` (five tools), `context_lines`, `budget`, `max_files`, `max_nodes`,
`resolution` — arrives unconstrained from a model-generated argument list
(**B22-M2**).

What that buys, all measured by block reviewers:

- `read_slice(lines=[[60,30]])` or `context_lines=-50` renders the **whole file** —
  the exact token blow-out the funnel exists to prevent, and the exact behaviour
  the docstring and the tool description promise never happens (**B17-H1**,
  **B12-H2**, **B22-M1**). The CLI validates the same input at `main.py:1383`.
- `analyze_structure(max_nodes=0)` escapes as a raw `ValueError` from
  `core/mermaid.py:253` rather than the `AtlasError` every other refusal on that
  surface uses (**B19-M1**). The CLI checks it at `main.py:819-820`.
- `resolution=nan` produces 110 singleton communities with `modularity NaN`, and
  `json.dumps` emits the bare token `NaN` — accepted by Python, rejected by every
  strict parser on the other end (**B11-H1**, **B19-M2**, reproduced end to end).
- `limit=0` on `find_symbol` / `find_referencing_symbols` returns "no matching
  symbols" / "no references" for a symbol with 26 matches and 52 sites — a
  confident false negative on the blast-radius tool (**B18-M2**, **B23-H4**).
- `limit=-3` on `expand_symbols` drops the last three ids and echoes the negative
  limit back into a user-facing receipt line (**B22** cross-block, `symbol_service.py:214`).

The CLI is only relatively better: it guards four of thirteen bound flags, and
`--limit 0` / `--depth -3` / `--max-files -1` are unguarded there too
(**B23-H4**, measured). So the honest statement is not "the CLI validates and the
MCP does not" — it is that **no layer owns numeric bounds**, and each of the three
candidate owners assumes one of the others did it.

**Fix direction.** One owner per bound, keyed on the invariant rather than the
front door. The service method is the boundary both adapters cross, so bounds
belong there (`core/mermaid.py:253` already chose this side for `max_nodes` and is
the model). Then annotate the wire parameters with `Field(ge=1, ...)` as *defence
in depth and schema documentation* — a schema refusal is the only message a model
can act on before the call — and replace the CLI's four hand-rolled guards with
`type=_positive_int` on every bound flag at once.

### S3 — Prose asserts an invariant the code contradicts, and no gate can catch it

**Blocks:** all 24. This is the single most-repeated finding shape in the audit.

The sync machinery in this repo is genuinely good: the prompts manifest validates
key sets eagerly, `test_prompts.py` reads a live FastMCP listing, the goldens are
byte-exact and deliberately regenerated, import-linter runs on commit. **Every one
of those gates checks structure. Nothing checks whether a sentence is true.**
B02 names this as its block theme and it generalises to the whole codebase.

The load-bearing instances — where the false sentence is what a maintainer or the
next agent will reason from:

| Claim | Where | Reality |
| --- | --- | --- |
| "serves those two directories and nothing under, beside or symlinked from them" | `repo_context.py:11-14` | client-advertised roots are unioned into the allowlist (**B16-H1**, **B22-H1**, both measured independently) |
| "checked, exactly, against the roots the server was started with" | `server.py:8-13` | withdrawn two sentences later in the same docstring (**B22-M5**) |
| "Static roots authorise; client roots select" | `server.py:96` | client roots authorise (**B16-H1**) |
| "Every string this package shows an agent, held as data" | `prompts/__init__.py:1` | violated at five sites, no gate (**B02-M2**) |
| "cannot serve a wrong answer" | `cache.py:22-30` | a stale snapshot returns `[]` symbols with a `fresh` receipt (**B08-C1**, reproduced) |
| "Readers never block" | `cache.py:39-45` | readers do not block; they `unlink` the database mid-index (**B08-H1**, reproduced) |
| "a read command never fails because of a cache" | `main.py:1300-1307` | two uncaught paths out of `open_source` (**B08-H3**) |
| "every tier-1 language has characterization coverage" | `grammars.py:38-40` | c/cpp/rust have no fixture and zero executed handler lines (**B07-H4**) |
| "Iterative rather than recursive" | `extractor.py:481-486` | four other walkers in the same file recurse per child (**B07-C1**) |
| "this module never opens a cache, a repository or a file of its own" | `patchlint.py:11-13` | `read_declared_dependencies` reads `pyproject.toml` (**B15-M6**) |
| "which can only make this check quieter and never make it accuse a real dependency" | `patchlint.py:397-415` | three of five probed inputs zero the declared set (**B15-M2**, differential vs `tomllib`) |
| "Nothing here reads a test command out of the repository under analysis" | `validate_service.py:29-33` | `main.py:1004` falls back to `.agentless-mcp.json` (**B21-M4**, **B23-H5**) |
| "every function body is replaced by a `...` sentinel" | `skeleton.py` header | a trailing comment on the first body line deletes the sentinel and truncates the function (**B12-H1**) |
| "neither adapter owns any behaviour of its own" | `main.py:1-14` | six subcommands do, and the layers contract permits it (**B23-M1**) |
| "the caps in force" | `tool_descriptions.json` + `agent-guide.md:573-580` | the MCP handler emits no caps at all (**B22-H4**) |
| "Every requested id comes back" | `symbol_service.py:17-18` | an unwarmed grammar raises out of the whole batch (**B18-H2**) |
| "Every one of them is bounded and says what it left out" | `graph_service.py:43` | the imports section is the counterexample in the same file (**B19-H1**) |
| "Reformatting, re-commenting and re-indenting all normalise away" | `normalize.py:12-13` | three of four things a formatter does move the key (**B13-M8**, measured) |
| "no cache or scratch state is written inside the repository" | `README.md:107-110` | `git worktree add` writes `.git/worktrees/<id>`, permanently on the kill path (**B14-H1**, reproduced) |
| "Both forms are built before either is chosen so `--json` cannot diverge" | `main.py:1322-1327` | `truncation` feeds one and is dropped by the other (**B16-M3**) |
| the two `load_candidates` "name the same candidate the same way" | `lint_service.py:87` | different collision and encoding behaviour (**B20-M3**, measured) |

Three of these (**B01-M3**, **B05-M3**, **B12-M4**) are worse than false: they are
*misdirecting*. `contained_path`'s dead strict re-resolve carries the comment
explaining the module's security property, so a maintainer deleting the live check
at line 44 as "redundant" removes the only containment test in the function.
`render.py:10-12` claims an ownership rule that, acted on, creates an import cycle
the layers contract will only catch after the work is done.

**Fix direction.** Two moves, and the cheap one first. (1) Correct the sentences —
that is a day's work and removes the class of defect where the next agent
*generates against* a false guarantee. (2) For the four or five invariants that are
genuinely load-bearing, convert prose into a gate: a test that asserts every
enumerated item in a tool description appears in a real answer (**B22-H4**); a test
that no string literal in `application/`/`core/` reaches an agent-facing channel
(**B02-M2**); a `forbidden` import contract for the adapter-owns-no-behaviour claim
(**B23-M1**); a subprocess `sys.modules` check for the optional-extra claim
(**B24-H3**). The global rule already states it: a stated guarantee names the gate
that enforces it.

### S4 — "Could not measure" is rendered as "measured, and the answer is negative"

**Blocks:** B06, B08, B13, B14, B15, B18, B19, B21, B23.

The most dangerous single class in this audit, because the output is confident,
plausible, and shaped exactly like a real answer. Every instance is a place where
infrastructure failure, absent evidence, or an unvalidated input produces a
negative *domain* verdict rather than a refusal:

- Three candidates whose test command never started (`RunStatus.ERROR`) rank at
  tier `applied` with a crowned winner and the detail "applied cleanly (nothing
  passed the regression suite)". Nothing was measured on any of them
  (**B21-H1**, executed against the real `vote` module). `ApplyStatus` has no
  `not_evaluated` member although `Verdict` does, so an UNVERIFIED run tells the
  vote "the patch did not apply" — false for every candidate (**B21-M1**, **B14-M5**).
- A `pyproject.toml` that did not parse yields `packages=frozenset()` with
  `known=True`, which is the state meaning "this repository declares nothing" — so
  every third-party import in the patch is reported as hallucinated, and the one
  warning that would explain it has no consumer anywhere in `src/`
  (**B15-H1**, reproduced; **B15-L2**).
- An ABI-incompatible grammar raises out of `Parser(...)`, outside every handler;
  `extractor.py:683,727` catch bare `ValueError` and log "Unsupported language",
  and every file in that language indexes to zero symbols while `capabilities`
  reports `probe_ok=False` for the same grammar (**B06-H1**).
- `syntax_delta` with no grammar returns `ok=True` ("syntax was not checked" lives
  only in a `detail` string `CheckReport.ok` never reads) (**B13-M6**).
- A stale `CachedSource` snapshot returns `[]` symbols with a `fresh` receipt, and
  "this file legitimately defines nothing" is a case the design deliberately
  records — so no downstream check can recover it (**B08-C1**, reproduced).
- `limit=0` renders "no references to X" (**B18-M2**, **B23-H4**).
- `ParseResult.ok` and `ApplyResult.ok` are both vacuously `True` on zero blocks,
  so a model refusal ("Sorry, I could not locate the bug") passes `patch check`
  with exit 0 (**B13-M3**, measured through the CLI).
- `patchlint.DEGRADED_ERRORS` names `TypeError`/`KeyError`/`IndexError`/
  `AttributeError` — the four classes a defect in that module raises — so a real
  crash becomes `not checked: the check could not run over this patch
  (AttributeError)` and the other six checks still produce a clean-looking report
  (**B15-M3**).
- `_vote_candidate` reads `record.get("reproduction")` with no presence check while
  every sibling field is refused if missing (**B14** cross-block, `validate_service.py:614-628`).

**Fix direction.** The vocabulary already exists in one place and should be
generalised: `Verdict.not_evaluated`'s docstring says it "is deliberately not
`failed`: nothing was measured, and a report that says otherwise is inventing
evidence." Make "not measured" a representable state in every result type that can
reach a ranking, a verdict, or an emptiness claim — `ApplyStatus`,
`SyntaxVerdict.checked`, `DeclaredDependencies.known`, a `CachedSource` miss, and a
zero-or-negative bound. A type that cannot express "unknown" will encode it as a
plausible answer every time.

### S5 — One piece of knowledge, two to four homes, already drifted

**Blocks:** B01, B03, B04, B05, B08, B09, B11, B12, B15, B16, B17, B19, B20, B22, B23.

Not a style complaint. In eleven of the cases below the copies have *already*
diverged, and in four of them the divergence is user-visible in a single response.

| Knowledge | Homes | Drift status |
| --- | --- | --- |
| the walk bound | `fslimits.py:141-177`, `treewalk.py:90-115` | message text already differs (**B01** x-block) |
| `N\|` line numbering | `render.py:767,857`, `skeleton.py:296,299`, `slices.py:53`, dead `render.number_lines` | three different formats (**B05-L1**, **B05** x-block) |
| line counting | `view_service.py:167,200`, `refs.py:244`, `slices.py:93` | identical and all wrong: phantom trailing line, and one agent-facing message calls it "the true line count" (**B17-M2**, **B12-M1**) |
| the elision count | `mermaid.py:203`, `graph_service.py:259` | different denominators; one response says 89 and 105 (**B11-M4**, **B19-H2**, both measured) |
| the symbol-source open | `server.py:202-210`, `main.py:1310-1318` | verbatim duplicate; both docstrings claim the decision is *theirs* (**B22-M3**, **B16-M4**) |
| `capabilities` rendering | `server.py:345-372`, `main.py:1143-1178` | already diverged: CLI has tier/extensions/config/caps, MCP has the index hint and the version (**B22-H4**, **B23-M2**) |
| `load_candidates` | `lint_service.py:87`, `validate_service.py:517` | different collision and encoding behaviour, and the lint docstring falsely claims parity (**B20-M3**, measured) |
| containment + canonicalisation | `patch_service.py:372-387`, `lint_service.py:146-158` | verbatim; one is a refusal boundary (**B20-M2**) |
| the file-graph pipeline | `map_service.py:125-134`, `graph_service.py:272-287` | hand-mirrored incl. stoplist; a map weighting change will silently not reach communities or diagrams (**B19** x-block) |
| the budget precedence rule | `server.py:216-217`, `main.py:1417` | agree today; the MCP surface cannot express `auto` at all (**B17-M5**, **B23-M2**) |
| `locate` the search block | `patchlint._locate`, `patches._apply_one` | already drifted: `_locate` lacks elision resolution, so elided edits lose their line numbers (**B15-M5**, reproduced) |
| per-file symbol index | `refs.symbols_by_qualname` (dead), `resolve.py:673`, `patchlint.py:928` | three different tie-breaks (**B09** x-block) |
| `GIT_TIMEOUT_SECONDS` | `gitinfo.py:28` (5.0), `treewalk.py:32` (30) | same name, 6x apart, no stated reason (**B04-L3**) |
| the three git readers | `gitinfo.head_sha/tree_oid/dirty_count` vs `snapshot:82-102` | byte-for-byte duplicate argv; only `snapshot` has production callers (**B04-M5**) |
| partition serialization | `communities.as_dict`, `render.py:459-484` | different semantics; the domain copy is test-only (**B11-M5**) |
| `omitted` | four copies in `render.py` | identical, and absent from the two listings that need it (**B05-H1**) |
| `qualname` | `tags.qualname` column (written, never selected) vs recomputed on read | one home unread and therefore untested for drift (**B03** x-block) |
| the tool manifest | `prompts.TOOL_NAMES`, `test_mcp_server.py:34-46` | (**B02-L9**) |
| `GRANULARITIES` | `projectconfig.py:55`, `map_service.py:45` | (**B17-L2**) |
| the locations renderer | `main.py:1354-1369`, `server.py:332-343` | (**B23-M2**) |
| the skeleton stable-id hint | `server.py:254-256` has it, `main.py:671` does not | already drifted (**B23-M2**) |

**Fix direction.** Rank by whether a divergence is *observable*. The five that
already produce contradictory user-visible output (elision count, capabilities,
line counting, skeleton hint, `load_candidates`) should be consolidated first. For
the two adapter-shared cases the import-linter independence contract forces the
shared home into `application/` — that is a constraint, not an obstacle:
`repo_context` already owns `RepoContext` construction and `render.py` already owns
rendering. For the rest, the rule to write down is the one this codebase keeps
almost getting right: a derived fact has one producer, and consumers read it rather
than re-deriving it.

### S6 — The guard exists where somebody thought of it, not where the invariant lives

**Blocks:** B04, B06, B10, B12, B13, B15, B17, B20, B23.

The global rule is "guards key on the invariant, not a proxy." Nine blocks found
independent instances, and two of them found the same *meta*-shape: a bug was fixed
in the caller and left live in the primitive.

Proxy guards:

- `is_git_repo(root)` tests "there is a `.git` entry at exactly this path"; the
  invariant is "this path is inside a git work tree." A package inside a monorepo —
  an explicitly supported root — silently loses gitignore and indexes
  `vendored/.git/config` (**B04-H1**, reproduced). `gitinfo.git_root` answers the
  real question and is in the same block.
- `shortest_path` filters on `edge.tier is not AMBIGUOUS`; the invariant is "was
  this binding guessed." 355 of 14,906 resolutions land on multiple candidates at a
  strong tier and are walkable by default (**B10-H3**, measured on this repo).
- `same_file` filters on "defined in this file" rather than "in this file's module
  scope", so a method shadows a declared import and the correct target is never
  even produced (**B10-H1**, reproduced).
- `_require_clean` reads `ctx.dirty_count`, snapshotted before an unbounded
  `sys.stdin.read()`; the invariant is "the tree is clean *now*" (**B20-M1**).
- `validate_service.py:503` keys the equivalence guard on "did bytes change"; the
  invariant is "did the structure change", so comment-only candidates all hash
  `sha256("")` and cluster together (**B13-H3**, **B13** x-block).
- `warmed_languages()` membership is canonical-name-only while `pack.has_language`
  accepts aliases, producing a remediation loop with no terminating input
  (**B06-H2**, measured).
- `_names_keyword_argument` is correct at index 0 only because `"" in "=!<>"` is
  `True` (**B15-L4**).
- `map_service` bounds are enforced on the *config* path and not the *request*
  path: a repo owner's committed config is checked more carefully than a live tool
  call (**B17-M1**).

Fix-in-the-caller:

- `b7a97ca` fixed the silent whole-file fallback in `view_service.read_slice`, not
  in `slices._clamp:130` (`return clipped or [(1, total)]`), so the trap is intact
  for the next caller — and the MCP path reaches it today (**B12** x-block,
  **B17-L7/B17-H1**).
- `_Plan.replace` is documented to *undo* another rule's decision, with no
  precedence statement anywhere; **B12-H1** and **B12-H3** are two instances of
  that one gap (**B12-M4**).

**Fix direction.** For each pair, ask which side can be stated as an invariant and
put the check there — `_clamp` should distinguish "no interval requested" from
"every interval was unsatisfiable"; `_Plan` should have an `owned` range set that
`replace` refuses to overwrite; the git branch should ask git. Where the guard
genuinely belongs at a call site, the primitive should be shaped so the unguarded
call cannot compile or cannot mean anything.

### S7 — Errors do not surface: untyped raises, over-wide catches, and adapters that catch one class

**Blocks:** B01, B02, B04, B06, B07, B08, B14, B15, B19, B21, B23, B24.

`adapters/cli/main.py:129` catches `AtlasError` and nothing else; `bootstrap.py:112`
catches `AtlasError` around counter selection. Everything else reaches the user as a
traceback. Twelve blocks found something that escapes that hierarchy:

- `contained_path` — the documented boundary parse step — raises untyped
  `ValueError` on a NUL byte (**B01-M1**).
- `loader` catches `OSError`, and `UnicodeDecodeError` is a `ValueError` (**B02-L1**).
- `treewalk._git_listed_paths` catches `FileNotFoundError`/`TimeoutExpired` but not
  `OSError`, which the sibling `gitinfo._run` does (**B04-M4**).
- `sandbox.worktree` leaks `ValueError` from `relative_to` (**B14-L4**).
- `load_verdicts` raises `AtlasError` with a line number everywhere except
  `BaselineStatus(...)`, which raises bare `ValueError` — measured as a traceback
  out of `agentless-mcp vote` (**B21-H3**).
- `read_text(encoding="utf-8")` guarded by `except OSError` at three CLI sites; a
  latin-1 byte produces a traceback *and* exit 1, which for `diagram --check`
  collides with the documented meaning "it has drifted" (**B23-H3**, measured).
- `mermaid` raises bare `ValueError` for `max_nodes=0`, straight out of an MCP tool
  (**B19-M1**).
- `mcp_main` reports *every* `ImportError` from a 684-line module as "the mcp extra
  is not installed" (**B24-H1**, verified).

And in the other direction, catches too wide to mean anything: `extractor.py:683,727`
turning any `ValueError` into "Unsupported language" + `[]` (**B06-H1**);
`patchlint.DEGRADED_ERRORS` swallowing its own defects (**B15-M3**); and the
inverse, `cache._plan_index` catching only `LanguageUnavailable` so one
`RecursionError` from a minified bundle aborts the whole-repository index with a
traceback (**B07-C1**, measured at 248 chained JS calls).

**Fix direction.** The typed hierarchy is finer than any consumer uses
(`formatting.py:57-68` maps five subclasses onto two exit codes with a dead second
branch — **B01** x-block, **B23-L2**), which is the tell that the hierarchy and its
mapping were designed separately. Decide the raise-surface per module and assert it:
every public entry point in `core/` and `application/` either raises `AtlasError`
or documents the stdlib type it propagates, and the adapters catch exactly that.
`patchlint.py:178-188`'s named-tuple approach is the right shape and should be
narrowed to what foreign input actually raises, then adopted by
`cache._plan_index`.

### S8 — Repository-controlled content bypasses the bounds that exist to contain it

**Blocks:** B01, B04, B14, B16, B22.

The package's security posture is "the repository is untrusted content." Five
blocks found a bound the repository can step around:

- A `.agentless-mcp.json` with 8,000 unknown keys produces 8,000 uncapped warnings
  in the header, which is subtracted from the budget rather than bounded by it:
  **301,789 tokens** from a `max_tokens=16000` call, with the entire body dropped
  and honestly marked as truncated. Every tool, every call, for that repository
  (**B16-C1**, measured; producer half **B04-H2**, measured at 394,890 chars).
- `walk_repo`'s git path serves files symlinked outside the root — `git ls-files`
  lists symlinks and the only filter is `is_file()`, which follows them. The same
  file is *refused* through `read_slice` and *served* through `repo_map`/`refs`/the
  index (**B04-C1**, reproduced). `projectconfig.load` has the same hole at the
  file level (**B04-M1**, reproduced).
- `bounded_walk`'s `max_files`/`max_bytes` are counted *after* the `include`
  predicate, so the bound is disarmed by the filter: 30,000 files walked with
  `max_files=10` in force (**B01-M5**, measured). Nothing bounds directory count or
  `seen_dirs` memory at all (**B01-M6**).
- `DEFAULT_MAX_CAPTURE` bounds only what is read back; the child writes into an
  unbounded `tempfile` in `TMPDIR` — often tmpfs — for the whole timeout window,
  multiplied by `2N` under `--jobs N`, with no ceiling on `--timeout` (**B14-H2**,
  **B14** x-block).
- Client-advertised MCP roots are unioned into the authorization allowlist with no
  containment check, and `file://` resolves to the server's cwd while
  `file://host/etc` silently becomes local `/etc` (**B22-H1**, **B22-H2**, both
  measured; independently confirmed by **B16-H1**).

**Fix direction.** Bound the producer *and* the consumer, and say so once. Cap
`projectconfig`'s warning list beside `MAX_STOPLIST_ENTRIES` (the asymmetry inside
one parser is itself the tell), and clamp the whole envelope header so no path can
emit more than `max_tokens`. Move the containment test to the *read*
(`refs.py:120`, `cache.py:645`) rather than to each of the three entry points that
currently remember to call it. Give `bounded_walk` a counter that measures work
done. And resolve the root-authorization model per ARCH1 below.

### S9 — A lossy read decision leaked into every write surface

**Blocks:** B01, B13, B20 (B20 independently confirmed B01 and B13).

`fslimits.py:105` returns `data.decode("utf-8", errors="replace")` with
`skipped=None`, and `BoundedRead`'s own docstring says a skipped file "must also
never pass silently as an empty file" — a promise kept for the size cap and for
`OSError`, and broken for encoding. There is no field a caller could consult to
learn the decode was lossy (**B01-H1**).

Verified this pass: 10 call sites, three feeding a write. Measured consequence:
`patch apply --in-place` on a repo whose `app.py` ends `# café` writes
`# caf\xef\xbf\xbd` — corruption in a region no edit named — and reports `ok=True`,
exit 0 (**B20-C1**). The same corruption lands in `validate_service.py:485`'s
worktree, so a candidate can be failed by damage the tool introduced (**B20**
x-block). `lint_service` reads candidates through the lossy reader while
`validate_service` refuses non-UTF-8 with a message (**B20-M3**).

The fix is not symmetric, which is why this needs one decision rather than three:
`cache.py:396-404` **deliberately depends** on the lossy behaviour so the indexer
and the reader hash the same text (**B01** x-block flag). A global switch to strict
breaks the cache; a global switch to lossy is the current bug.

**Fix direction.** Two readers with two names — `read_bounded` (lossy, analysis)
and a strict variant that turns `UnicodeDecodeError` into an `unreadable` entry —
and route every write path and every candidate load through the strict one.
`normalize.py:207` has the same `errors="replace"` in an equivalence key
(**B13-L3**), which is minor on its own and belongs to the same decision.

### S10 — "Language-neutral" is asserted per feature and verified for Python

**Blocks:** B03, B06, B07, B10, B12, B13, B18.

The tool advertises 15 languages in two tiers, and the tier split is load-bearing
(it is what lets a broken tier-2 grammar degrade without failing tier-1 warmup).
Measured reality:

- `c`, `cpp` and `rust` are tier-1 with **no fixture anywhere** and zero executed
  handler lines under the full suite — strictly less verification than every
  tier-2 language (**B07-H4**). The claim of "characterization coverage" is in
  `grammars.py:38-40` (**B06** x-block).
- Go `type_declaration` extracts nothing — `class_node_types` is dead
  configuration — and the golden `repo_go.map.json` has zero type symbols while
  `test_stable_ids.py:117-120` asserts the absence. Fixing the extractor turns that
  test red (**B07-H1**).
- C/C++ extraction sees root-level declarations only: `ns`, `Foo`, `Foo::bar` and
  `ns::free_fn` are all dropped (**B07-H6**, measured).
- Non-Python import handlers ship the contradictory pair `is_relative=True,
  relative_level=0`, and three consumers read three different notions of the same
  property (**B03-M1**, **B09-H1**).
- `Relation.INHERITS` is Python-only; every other handler constructs `bases=()`
  (**B10** x-block).
- `INDENT_BLOCK_NODE_TYPES` is `{"python": ("block",)}` with a `.get(..., ())`
  fail-open, so the dedent case the equivalence key exists to catch is unchecked
  for every other whitespace-significant language (**B13-M9**).
- `_defined_in_tests` promises language-neutral test-tree detection and delivers
  Python/Java: `foo_test.go`, `foo.test.ts`, `__tests__/`, `spec/` all read as
  production (**B18-M3**, measured against the conventions of the 15 supported languages).
- tree-sitter columns are byte offsets sliced as character indices, so any
  non-ASCII on a line corrupts the skeleton — and the fixture corpus contains zero
  non-ASCII characters (**B12-H4**, **B12-M6**).
- Five language tables (`ALL_LANGUAGES`, `_PROBE_SAMPLES`, `LANGUAGE_CONFIGS`,
  `SUPPORTED_EXTENSIONS`, `LANGUAGE_PREFIXES`) agree today with nothing keeping them
  that way (**B06-M4**).

**Fix direction.** Make tier membership a testable claim: one gate asserting a
characterization fixture exists per tier-1 language, and one asserting set equality
across the five language tables in both directions. Then either add `repo_c` /
`repo_rs` fixtures or demote `c`/`cpp`/`rust` — the comment must not outlive the
evidence. Separately: **audit the goldens against hand-written expectations of what
each fixture declares**, because they were generated from the implementation and
therefore encode its blind spots (**B07** x-block).

### S11 — Dead public API, and dead defensive code that misdirects

**Blocks:** all except B14 and B21 (which have their own single instances).

Roughly 40 findings. The volume matters more than any one item: it is what makes
the codebase read as larger and more capable than it is, and three instances are
actively harmful rather than merely inert.

Harmful (a reader draws the wrong conclusion): `contained_path`'s unreachable
strict re-resolve carries the security rationale (**B01-M3**); `RefGraph`'s
`setdefault` makes one invariant violation invisible while the symmetric one raises
`KeyError` from inside a numeric loop (**B09-M5**); `_MODULE_SUFFIXES` ends with
`""` with no stated case (**B09-L7**).

Largest by volume: the entire `intervals` machinery on the write path — ~60 lines
plus a public `EditStatus` enum member plus a 70-line test class, with no caller
anywhere in `src/` (**B13-M7**, **B20-M4**); `patchlint`'s manifest reader is a
separate module wearing another's name (**B15-M7**).

Notable singletons: `extractor.extract_symbols(Path)`/`extract_imports(Path)` —
callerless, unbounded reads, and the only paths that produce absolute-path
non-portable stable ids (**B03-M3**, **B07-M5**); `gitinfo.head_sha`/`tree_oid`/
`dirty_count` (**B04-M5**); `grammars.load_language` (**B07-L4**);
`refs.symbols_by_qualname` (**B09-M2**); `render.number_lines` (**B05-L1**);
`SymbolKind.DECORATOR` in a wire enum read by 13 modules (**B03-M5**);
`ImportStatement.resolved_path`, carried through a `NOT NULL` column
(**B03-M2**); `cache.head_sha`, six touch points and a column (**B08-L2**);
`validate_service.as_vote_candidate`, which is also a second home for the
eligibility rule (**B21-M3**); `DeclaredDependencies.warnings`, written and never
read (**B15-L2**); `PatchService.parse`, a one-line wrapper whose only
distinguishing property is being wrong (**B20-M5**).

**Fix direction.** One deletion pass, done as its own commit so it is reviewable.
Prefer deletion to documentation: this is a prototype-internal surface, so a hard
removal beats a shim. The four "dead defensive branch" cases should be deleted
*and* the comment they carry moved onto the live check.

### S12 — Concurrency contracts are stated in prose and owned by nobody

**Blocks:** B06, B08, B14, B20, B21.

- The cache database has two mutators: `build_index` under `write.lock`, and
  `_discard` — called from the **read** path — with no lock at all. A reader
  unlinks the database and its WAL out from under an in-progress index run, which
  then reports success having written nothing (**B08-H1**, reproduced). "One owner
  per state change" fails for the one file the module is about.
- `CachedSource` decides freshness from a snapshot and reads rows in separate
  autocommit statements, with no `BEGIN` pinning a WAL snapshot across the two
  (**B08-C1**, reproduced). No test in the repo covers a reader concurrent with a
  writer — `TestSingleWriter` is writer-vs-writer (**B08-M7**).
- `sandbox` guards a documented *inter-process* race (`worktree add` vs `prune`)
  with a `threading.Lock` (**B14-M1**).
- `grammars.get_parser` is `@cache`d, so every `--jobs N` thread shares one mutable
  `Parser`. Measured safe today only because the GIL is held across `parse` —
  0 anomalies, 1.00x speedup — i.e. the safety rests on an implementation detail
  nobody wrote down (**B06-L6**, **B21-M5**, measured independently).

**Fix direction.** Name the owner per resource and use the primitive that matches
its scope: `util.filelock` (repository-scoped) for the worktree bookkeeping and for
`_discard`; an explicit `BEGIN` held for the source's lifetime — sources are
per-request and short-lived, which is the right scope — for the read snapshot; and
either a thread-local parser cache or a fresh `Parser` per call (construction is
free next to a test-suite run). Then pin each with the interleaving test that does
not exist.

---

## Part 2 — [COUPLING]

Boundary mismatches: two blocks that each look correct in isolation and disagree
about the shape or meaning of what crosses between them.

**C1 — `BoundedRead` serves three incompatible requirements through one type.**
`fslimits` (B01) produces lossy text with `skipped=None`; `cache.content_digest`
(B08) *requires* the lossy form so indexer and reader agree; `patch_service`
(B20) writes it back to the user's checkout. One type cannot satisfy all three, and
today the write path loses. **B01-H1**, **B08** (`cache.py:396-404`), **B20-C1**.

**C2 — `FileSource` advertises `receipt` as a cheap property; it is four unmemoized
`COUNT(*)` scans, one of them over the largest table.** `envelope.py:63` evaluates
it 2+ times per response and the CLI's `_Answer` builds both forms
unconditionally — at least eight full table scans per command, for five fields that
touch no row. **B08-M2**, **B16**, **B08** x-block.

**C3 — `ImportStatement`'s relativity is encoded three ways and read three ways.**
The extractor strips the dots (`from ..pkg.deep import b` -> `module='pkg.deep',
relative_level=2`); `graph.py:250` gates on `module.startswith(".")`, so the only
branch that understands `relative_level` is unreachable — confirmed by coverage
under the full suite. `patchlint.py:861` and `graph.py:243` each key on something
different again. Every Python relative import silently loses its 3x-weighted edge.
**B03-M1**, **B09-H1** (both measured).

**C4 — `end_line_number is None` has two contradictory meanings across one block
boundary.** `slices.py:147`: "encloses everything after" (so it becomes a sticky
header for every later slice). `locs.py:307`: "a one-line span." Reachable from a
cache row written by an older build, since `cache.py:837` decodes NULL to `None`.
**B12** x-block, **B08**.

**C5 — The write side's signature cannot represent the invariant the parse side
exists to protect.** `check`/`apply`/`normalize` take `Sequence[Edit]`; `load_edits`
returns a `ParseResult` carrying `errors`. `patches.py:112-117` states the rule
("a truncated patch would otherwise apply its first half and report success") and
the type erases it, so three consumers hand-roll the guard and one forgets.
Measured: exit 0, half a patch on disk. **B20-H4**, **B13** x-block.

**C6 — `ApplyStatus` and `Verdict` have asymmetric vocabularies.** `Verdict` has
`not_evaluated`; `ApplyStatus` has only `OK`/`FAILED`, so an UNVERIFIED run stamps
`FAILED` on every candidate and `vote._exclusion` reports "the patch did not
apply" for patches never attempted. The honest reason survives in `apply.reasons`,
which `_vote_candidate` never reads. **B21-M1**, **B14** x-block.

**C7 — `check` and `apply` disagree about the same repository.** `_apply_at` drops
`_Sources.unreadable`, so `apply_edits` sees no entry and emits `no_such_file` for
a 1.2 MB tracked file that `check` correctly reported as over the cap. The two
halves of the advertised pipeline give contradictory answers about one input.
**B20-H1** (measured).

**C8 — The stable-id vocabulary and the location grammar have drifted apart.**
`locs.py` can address `FUNCTION`/`METHOD`, `CLASS_KINDS` and `CONSTANT`;
`TYPE_ALIAS` has no location form at all, and `_resolve_class`/`_resolve_variable`
ignore the ordinal that `_resolve_function` splits off. The CLI then re-encodes
*every* id as `f"function: {qualname}"`, so half the id vocabulary answers "no such
symbol" with exit 0 — while `expand`, `explain` and `refs` accept the same id.
**B03-H2**, **B12-M2**, **B23-H1** (all measured).

**C9 — `duplicate_index` is produced kind-agnostically and consumed kind-first.**
`disambiguate` keys on `(parent_class, name)`; `locs.py:264` compares the ordinal
after filtering to one kind. A file with `class Foo` and `def Foo` makes the
function unaddressable, with a self-contradicting refusal ("defines 1 symbols named
'Foo', so there is no number 1"). **B03-H1** (measured).

**C10 — `DiagramView.elided` and the diagram's own elision marker are computed
against different candidate sets.** The service uses every node in the repository;
the renderer uses the focus neighbourhood. One response says "12 elided" over a
picture that dropped nothing, with a caveat blaming a rank bound the service cannot
observe. **B19-H2**, **B11-M4** (both measured).

**C11 — `wrap_json`'s `**payload` is last, so a service key silently overwrites the
receipt, the untrusted-content notice, or the truncation report.** Latent — no
service collides today — and `receipt`/`notice`/`truncated` are not unusual field
names. **B16-H2** (measured).

**C12 — `RefGraph` accepts edge maps its own consumer cannot process, failing two
different ways depending on which endpoint is unknown.** An unknown *source* is
silently dropped; an unknown *target* raises `KeyError` from inside the power
iteration. `build_graph` maintains the invariant; `mermaid`, `communities` and four
test modules construct `RefGraph` directly. **B09-M5** (measured).

**C13 — The honest-truncation channel does not exist on the JSON side.** `wrap`
takes `truncation`; `wrap_json` has no such parameter, so `--json` can only report
the *ceiling*, never what the service left out. Combined with S1, text and JSON
disagree about honesty in a fixed direction. **B16-M3**, **B18** x-block.

---

## Part 3 — [DRIFT]

Conflicting conventions: cases where the codebase has decided the same question
more than once, differently, and no decision is marked as the one.

**D1 — Three git-degradation policies.** `gitinfo` turns every failure into a note
and answers anyway; `treewalk` turns git-absent into a hard `RepoResolutionError`;
`sandbox.py:386` carries a comment acknowledging it is a third. Consequence: on a
machine without git, every non-repo directory works and every repository fails, and
which you get depends on which module the command routed through. `git_root` then
drops the note entirely, so both callers assert the specific cause and can be
wrong. **B04** x-block, **B04-M2**.

**D2 — Three parse-failure policies for one `load_edits`.** `validate_service.py:470`
refuses the candidate; `lint_service.py:129-141` downgrades to a `NOT_CHECKED`
finding; `main.py:1218-1221` notes to stderr and applies the surviving edits with
exit 0 (measured: half a patch on disk). Only one can be right. **B13** x-block,
**B20-H4**, **B20** x-block.

**D3 — Exit codes are a documented three-value contract honoured by roughly half
the CLI.** `formatting.py:9-16` states the rule; `skeleton`/`expand`/`find-symbol`/
`slice --symbol` return `EXIT_OK` for "no such symbol" and "unreadable file" while
`slice` and `explain` return `EXIT_DOMAIN` for the identical condition
(**B23-H2**, measured). `vote` returns `EXIT_OK` at `TIER_NONE` and on UNVERIFIED
runs (**B14** x-block). `lint` returns `EXIT_OK` by design but `EXIT_USAGE`/2 when
one candidate raises (**B20** x-block). `patch apply` returns `EXIT_DOMAIN` on
failed edits and `EXIT_OK` when only the *parse* failed. And `exit_code_for` maps
five error subclasses onto two codes with a dead second branch (**B01** x-block,
**B23-L2**), so `CacheLocked` and bare `AtlasError` are indistinguishable to a
script.

**D4 — Four different homes for "where a bound is validated."** In core
(`mermaid._validate`) / in the CLI only (four of thirteen flags) / on the config
path but not the request path (`map_service`) / nowhere at all (`detect_communities`,
`symbol_service`, the entire MCP wire). `communities` and `mermaid` are siblings in
one block and disagree. **B11-H1**, **B17-M1**, **B19-M1**, **B22-M2**, **B23-H4**.

**D5 — Two `--json` document shapes.** Every read subcommand routes through `_emit`
-> `wrap_json` and begins `{"receipt": ..., "notice": ...}`; `diagram --json` and
`index --json` emit `json.dumps(as_dict())` bare. A reader keyed on
`document["receipt"]["head"]` raises `KeyError` on exactly the two outputs most
likely to be cached. **B23-M3**.

**D6 — Two `load_candidates`, same name, same documented rule, different
behaviour.** Stem collisions (validate refuses, lint emits two rows with the same
id), encoding (validate refuses non-UTF-8, lint mangles it), shape (file vs
directory — this one deliberate). **B20-M3** (measured).

**D7 — Per-file degradation is a convention followed inconsistently.**
`refs._parse_one` catches `LanguageUnavailable` per file "so the scan degrades that
file alone"; `symbol_service._expand_one` does not, so one unwarmed grammar raises
out of a ten-id batch and discards the nine cards already built (**B18-H2**,
reproduced). `cache._plan_index` catches only `LanguageUnavailable` while
`patchlint` has a nine-type `DEGRADED_ERRORS` tuple with a written rationale, so a
`RecursionError` kills the whole index (**B07-C1**, **B07** x-block).

**D8 — Recursion discipline.** `extractor.walk_nodes` and `resolve.py:733` both
document an explicit iterative choice for the same reason; four walkers in
`extractor.py` recurse per child anyway. The pattern is known and unevenly applied.
**B07-C1**, **B07** x-block.

**D9 — The warnings channel has three postures.** `projectconfig.warnings` reaches
the user through `envelope.py:85`; `patchlint.DeclaredDependencies.warnings` has no
consumer in `src/` (and its absence is what makes **B15-H1** silent);
`patchlint.LintReport` has no field for them. Every `warnings: tuple[str, ...]` in
`core/` needs a delivery path or a deletion. **B15** x-block, **B15-L2**.

**D10 — Two `GIT_TIMEOUT_SECONDS`** (5.0 and 30), same name, same policy, same
block, no stated reason — with no aggregate bound, so `resolve_repo` plus a
`tree`/`map` can block 50 s on a call documented as bounded by a courtesy timeout.
**B04-L3**, **B04-M6**.

**D11 — Three stderr prefixes** (`agentless-mcp: `, `# `, bare indent) on a surface
whose premise is programmatic consumption. **B23-L4**.

---

## Part 4 — [ASSUMPTION]

Beliefs multiple blocks hold about another block's behaviour, which that block does
not provide. These are the ones that will re-break after a local fix, because the
consumer's code encodes the belief.

**A1 — "The adapter validates."** Held by `map_service`, `view_service`,
`symbol_service`, `graph_service` and `communities`, all of which slice, cast or
clamp caller-supplied numbers without checking them. False for MCP entirely and
false for the CLI on nine of thirteen flags. Four blocks flagged it independently
before this pass counted the `Field(` uses. **B12** x-block, **B17** x-block,
**B18** x-block, **B19** x-block, **B22-M2**.

**A2 — "The receipt is cheap."** `envelope.py:63` reads `ctx.symbols.receipt` from
two places per response because `FileSource` advertises it as a property. It is
four `COUNT(*)` scans. **B08-M2**, **B16**.

**A3 — "`resolve_import_target` handles relative imports."** Held by
`resolve.py:306` and `:348` (ImportScope construction), by `map_service`'s ranking,
and by the `IMPORT_EDGE_WEIGHT = 3.0` design argument. It does not: the Python
branch is unreachable and the JS branch never normalises `..`. `resolve.py`'s
resolution tiers may currently be *tested against the broken resolver*.
`patchlint.py:829` is the one contained consumer. **B09-H1**, **B09-H2**, **B09** x-block.

**A4 — "`--root` is a confinement boundary."** Held by `repo_context.py:11-14`, by
`server.py:8-13`, by `--root`'s own help text, and by `docs/agent-guide.md:136-140`.
Two blocks measured the opposite independently, including the case of a server
started with **zero** `--root` flags serving whatever the client advertises.
**B16-H1**, **B22-H1**.

**A5 — "Anything `walk_repo` returned is inside the root."** Held by `refs.py:120`
and `cache.py:645`, which call `read_bounded(root / walked_path)` with no
containment check. False for any tracked symlink. **B04-C1**, **B04** x-block.

**A6 — "`read_bounded`'s text is what was on disk."** Held by every write path.
**B01-H1**, **B20-C1**.

**A7 — "The prompts catalog owns every agent-facing string."** Held by anyone told
to reword output without editing Python. Violated at `symbol_service.py:214,386`,
`render.py:588,709` and `cache.py:107-108,142` — the sharpest being a hand-written
refusal three lines from a catalog-sourced one in the same function. **B02-M2**.

**A8 — "The goldens encode correct behaviour."** They were generated from the
implementation, so they encode its blind spots, and at least one unit test now
*enforces* a defect (`test_a_plain_function_keeps_no_owner` asserts the absence of
every Go type). Fixing the extractor turns the suite red. **B07-H1**, **B07** x-block.

**A9 — "Line coverage is behaviour coverage."** The single most common reason a
defect in this audit survived a 90%-coverage suite. Concrete instances: the
skeleton corpus contains no trailing comments, nested functions, non-ASCII, tabs or
CRLF, so 99% coverage says nothing about the four High findings in that block
(**B12-M6**); the grammar-version freshness gate can be deleted and 52/52 stay
green (**B08-H2**, revert test); `test_a_section_limit_is_reported_rather_than_silent`
cannot fail as written (**B19-M3**); self-edge suppression can be deleted with the
suite green (**B10-M3**); four tests in `TestLoadedData` assert stdlib guarantees or
properties enforced at import (**B02-L8**); two tests in `test_graph.py` cannot fail
(**B09-L8**); two change-detector tests restate the constants they import
(**B22-L5**); `test_exit_codes_are_the_three_documented_values` restates three
constants while **B23-H2** shows the contract is broken (**B23-L7**).

**A10 — "`parsed.edits` is the whole patch."** Held by the CLI write path. See D2.

**A11 — "A green suite is evidence about the platforms we support."** There is no
CI. `requires-python = ">=3.10"` is exercised by no interpreter, and the only
`sys.version_info` gate in `src/` guards `patchlint`'s hand-rolled TOML scanner —
which this block measured disagreeing with `tomllib` on **this repository's own
`pyproject.toml`** (**B15-M2**). The Windows branches are uncoverable, and
`test_cache.py` cannot even be *collected* on Windows because of a module-scope
`import fcntl` (**B08-M6**). `tests/conftest.py:32-45` downloads grammars over the
network on a cold cache, session-autouse, for the whole suite (**B06** x-block).
Flagged independently by **B05**, **B15** and **B24**.

**A12 — "`validate_service` does not read a test command from the repository."**
Its own docstring says so in the strongest security language in the package; the
CLI's `_cmd_validate` falls back to `.agentless-mcp.json`, and the stated mitigation
is a stderr line written microseconds before `Popen`. **B21-M4**, **B23-H5**,
**B14-M3**.

**A13 — "`git worktree add` is a pure function of its argv."** `sandbox.py:26-28`
says nothing derived from repository content reaches git; git runs the target
repository's hooks and honours its config, verified with a `post-checkout` hook.
The author knew — `diff()` twelve lines later neutralises `color.diff` and
`diff.external` for exactly this reason — and applied the hardening to one call.
**B14-M2** (verified).

---

## Part 5 — [ARCH]

### ARCH-0 — Reconciling the Pass-2 design philosophy

Read across all 24 blocks, the stated philosophy is unusually coherent and unusually
well argued. Five theses recur, each defended in prose at the point of use:

1. A bounded view must say what it left out; a bounded view mistaken for a complete
   one is the failure the package exists to prevent.
2. Unknown is a value with a reason attached — degradation is reported, never
   silently coalesced.
3. Foreign data crosses one parse step that yields a typed value or refuses.
4. The receipt makes a wrong-repository or stale answer detectable.
5. Determinism is a property of explicit ordering rules, not of dictionary
   iteration.

The audit's structural finding is not that the philosophy is wrong. It is that the
philosophy is implemented as **prose far more consistently than as mechanism**, and
that the two have drifted in a predictable direction. Where a thesis has a gate it
holds: the layers contract holds, the prompts manifest sync holds, the goldens
catch rendering drift, the token ceiling holds for the body. Where a thesis lives
only in a docstring, S1 through S12 are the record of what happened — thesis 1 is
violated in six listings, thesis 2 in nine blocks, thesis 3 at every numeric
boundary, thesis 4 by the client-roots widening, and thesis 5 survives only because
nobody has reordered a dict yet (**B11-L1**).

So the highest-leverage architectural work in this repo is not a refactor. It is
**converting four or five load-bearing sentences into gates**, and deleting the
sentences that cannot be gated. Everything in ARCH-1 through ARCH-4 below is a
special case of that.

### ARCH-1 — The two-adapters problem: ruling

**The claim.** `adapters/cli/main.py:1-14`: "both adapters call the same
application services and neither owns any behaviour of its own."

**Measured this pass.** `main.py` imports 13 `core` modules directly (`cache`,
`communities`, `grammars`, `projectconfig`, `resolve`, `vote`, `extractor`,
`gitinfo`, `locs`, `mermaid`, `patches`, `symbols`, `treewalk`);
`server.py` imports 6. The layers contract is `type = "layers"`, which **permits**
`adapters -> core`. There is no `forbidden` contract.

**Ruling: the claim is false and, more importantly, unenforced.** Six CLI
subcommands own behaviour with no MCP counterpart — `index`, `warmup`, `vote`,
`capabilities`, `slice --symbol`, `diagram --check` — and three more
(`lint`, `patch *`, `validate`) are CLI-only by documented design, which is fine
but should be stated as such rather than contradicted by the module docstring
(**B23-M1**, **B23** x-block table). `communities.DEFAULT_RESOLUTION` is the
clearest symptom: imported into the adapter solely to interpolate a number into a
help string, while the actual default is resolved inside `GraphService` — the
adapter reaches into core to *describe* a decision it does not make.

The cost is already paid, three times over. `_with_source` / `_context` are a
verbatim duplicate, each with a docstring asserting the decision belongs to *its*
adapter (**B22-M3**, **B16-M4**). The `capabilities` renderings have diverged in
both directions — the CLI has `tier=`, extensions, config and caps; the MCP has the
version line and the index hint, and neither has what the shared tool description
and the agent guide promise (**B22-H4**, **B23-M2**). `resolve_locations`'s
line-assembly, the skeleton stable-id hint, and the budget precedence rule are each
written twice, and the skeleton hint has already drifted (**B23-M2**).

The `independence` contract is correct and should stay: either adapter importing
the other would drag `fastmcp` into every CLI install or give the server argparse
exit-code semantics. That constraint **forces the shared home into
`application/`**, which is where it belongs anyway.

**Fix direction.**
(a) Add a `forbidden` contract — `agentless_mcp.adapters` must not import
`agentless_mcp.core.{cache, communities, vote, grammars}` — and move `index`,
`warmup`, `vote` and `capabilities` behind services. That is the gate the docstring
has been standing in for.
(b) Move the three shared pieces one layer down: `with_symbol_source(ctx, extractor,
*, no_cache)` into `repo_context`; `render_locations` / the capabilities body /
the skeleton block renderer into `render.py`; budget precedence into `MapService`
with an explicit `AUTO` sentinel so `None` stops meaning two things (**B17-M5**).
(c) One test that asserts both adapters produce the same body for the same context,
which is the only thing that will keep them together.
(d) Whatever the CLI keeps, say so plainly in the docstring with the reason.

### ARCH-2 — The intra-core layering gap: ruling

**Measured this pass.** The layers contract names `core` as one opaque member, so
**no intra-core direction is constrained at all**, and `exclude_type_checking_imports
= true` would excuse a Protocol import anyway.

Three consequences, each found independently:

- `core/refs.py:28` imports `FileSource` *and* calls the concrete
  `effective_source` factory from `core/cache.py` at runtime. Dependency inversion
  is applied to the type (the Protocol is structural and a test substitutes for it)
  and not to the module: `import agentless_mcp.core.refs` transitively imports
  `sqlite3`, and `graph`, `resolve`, `patchlint`, `communities`, `mermaid` and all
  four application services inherit it. **B09-M1**, **B09** x-block.
- `core/normalize.py` and `core/skeleton.py` import four node-type tables from
  `core/extractor.py`, and `core/patchlint.py` reads them too. Those tables are
  **correctness-load-bearing for the equivalence key** — a change to
  `COMMENT_NODE_TYPES` changes what patches cluster as identical — while living in
  a module they have nothing to do with. This is the main driver of `extractor`'s
  Ca=14 and the reason any extractor split breaks two other blocks. **B07-M6**,
  **B07** x-block, **B13** x-block.
- `core/cache.py` imports `prompts`. **Ruled defensible on dependency grounds** by
  both B02 and B08 independently: `prompts` is stdlib-only, introduces no cycle,
  is the most stable module in the package, and its position below `core` is an
  explicit gated decision with a written rationale (`pyproject.toml:255-267`). The
  real defect is in the consumer — `cache._rejection` returns agent-facing prose
  *and* uses it as the control-flow sentinel for "is this database usable", so a
  presentation validation rule is load-bearing for persistence correctness
  (**B02-M4**) — and in the content: core renders `--no-cache`, a CLI flag, into
  every MCP receipt (**B02-M1**, **B08-L6**).

**Fix direction.** Declare an intra-`core` layer order in the same contract file —
roughly `patchlint/resolve/communities/mermaid -> graph -> refs -> cache ->
extractor -> grammars/symbols/imports` — which the modules already respect in every
case except `refs -> cache`. Two mechanical moves make the contract satisfiable and
unblock the two god-module splits: (i) `FileSource` + `OnDemandSource` +
`effective_source` into a neutral module below both; (ii) the node-type vocabulary
into `core/nodetypes.py`. Both are pure moves with no behaviour change, and (ii) is
the single highest-value mechanical refactor in the dependency graph. Only then
split `extractor.py` (2090 LOC) and `patchlint.py` (1761 LOC), and gate the
patchlint split on first pinning its string/comment scanner, which nothing executes
today.

### ARCH-3 — The composition-root hole: ruling

**Measured this pass.** `bootstrap` and `__main__` are outside the layer list by
explicit decision; the `independence` contract names `agentless_mcp.adapters.cli`
and `agentless_mcp.adapters.mcp` and not `bootstrap`. **Nothing constrains what the
composition root imports.**

The optional-extra guarantee — the thing that lets `pip install agentless-mcp`
without extras produce a working CLI — therefore rests on
`tests/unit/test_package.py:36-39`, which asserts that the strings `"import
fastmcp"` and `"from fastmcp"` do not appear in one file. The most likely way a
future edit breaks the invariant is
`from agentless_mcp.adapters.mcp.server import ServerServices` — which contains
neither string, and which is the obvious careless fix for **B24-H2** (**B24-H3**).

Two more holes in the same place. `mcp_main` imports the entire 1424-line CLI
adapter at module scope for three names it does not use, so the composition root
holds one half of the adapter-independence symmetry with real machinery and breaks
the other half by accident (**B24-M2**). And the dynamic import's wiring —
`module.ServerServices(...)`, `module.serve` — is `Any` to mypy (verified: a probe
constructing `ServerServices(bogus_field_that_does_not_exist=1)` type-checks clean
under this repo's own strict config) *and* executed by zero of 1141 tests, so field
drift ships a console script that dies at startup with every gate green
(**B24-H1**, **B24-H2**).

**Fix direction.** Add `forbidden: agentless_mcp.bootstrap -> agentless_mcp.adapters.mcp`
— enforceable today, and it replaces the substring test with a real gate. Replace
the remaining substring assertions with a subprocess `sys.modules` check (a fresh
interpreter is required; the in-process module table is polluted by other tests).
Split `cli_main` and `mcp_main` so neither imports the other adapter, with the
shared four-service construction in a private helper that imports neither. Add one
test that invokes `mcp_main` with `serve` patched to a recorder, which pins the
constructor arity, the keyword names and the call shape in three lines. And key the
missing-extra guard on `find_spec("fastmcp")` rather than on "any `ImportError`
from a 684-line module."

### ARCH-4 — No CI, so every "enforced by" in this repository is conditional

Verified: `.github/` does not exist. `ruff`, `mypy`, `lint-imports` and `deptry`
run as pre-commit hooks — i.e. enforced if and only if the contributor installed
them. Flagged independently by **B05**, **B15** and **B24**.

This is not a process nit; it is why several findings above are Low rather than
High. `requires-python = ">=3.10"` is exercised by no interpreter, so
`render.py:619`'s load-bearing `chr(10)` (a `SyntaxError` on 3.10/3.11 if
"cleaned up") is unguarded (**B05-L6**), and `patchlint`'s 3.10-only TOML scanner —
which measurably disagrees with `tomllib` on this repository's own manifest — has
never run in any test (**B15-M2**). The Windows dispatch code exists specifically so
this package runs there, and `test_cache.py` cannot be collected on Windows at all
(**B08-M6**). `tests/conftest.py:32-45` performs a real network download on a cold
cache, session-autouse, so a first run on a fresh box is not hermetic (**B06** x-block).

**Fix direction.** A minimal matrix (3.10/3.11/3.13 x linux + one windows job)
running `ruff`, `mypy`, `lint-imports`, `deptry` and `pytest`, plus one job that
installs the package **without extras** and runs `agentless-mcp map` — that last one
is the gate ARCH-3 is missing. Until it exists, every guarantee statement in the
codebase should be read as an intention.

---

## Recommended sequencing

Ordered so each step makes the next one safe, not by severity.

1. **Gates first, since they are cheap and they hold everything else in place.**
   CI (ARCH-4); the `forbidden` contracts for `bootstrap -> adapters.mcp` and
   `adapters -> core.{cache,communities,vote,grammars}` (ARCH-1, ARCH-3); the
   subprocess optional-extra check.
2. **Decide the four open policy questions and write each answer down once.** The
   root-authorization model (A4/S8); the parse-failure posture (D2); the exit-code
   contract (D3); where numeric bounds live (S2/D4). Each is currently answered two
   or three ways, and no local fix survives without the ruling.
3. **The mechanical moves that unblock everything else.** `core/nodetypes.py`; the
   `FileSource` neutral module; `with_symbol_source` into `repo_context` (ARCH-2, ARCH-1).
4. **The two silent-wrong-answer classes**, in this order: the truncation listing
   type (S1), then the lossy-decode split (S9) — the second is a data-loss bug but
   is narrower and needs no design decision beyond "two readers, two names."
5. **The correctness clusters**, each of which is now a normal bug fix: relative
   imports (C3/A3), the concurrency ownership (S12), the not-measured vocabulary
   (S4), the language-coverage gates (S10).
6. **The deletion pass** (S11), as its own reviewable commit, last — it is the
   cheapest work and the easiest to review against a tree that has stopped moving.
7. **The prose correction pass** (S3) alongside step 6, deleting any sentence that
   cannot be gated rather than restating it.
