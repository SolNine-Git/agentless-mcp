# Changelog

## 0.7.0 -- 2026-08-25

Responses are text and nothing else, and a focused map spends its budget on
what the focus reached.

### Changed

- **`structuredContent` is gone; read `content[0].text`.** Every handler
  returns `str`, and FastMCP's default for a non-object return type is to
  generate a wrapping output schema and emit
  `structured_content={"result": <the string>}` beside the text block. That
  copy was never a structured view of the answer: it held one field carrying
  the whole receipt as an escaped string, so a client that prefers structured
  content rendered a multi-line receipt as a single line with `\n` and `\"`
  in it, and every answer crossed the wire twice. All fourteen registrations
  now pass `output_schema=None`. A client that read `structuredContent.result`
  must read `content[0].text`; both carried identical text, so the migration
  is the field name and nothing else. This is why the minor version moves
  rather than the patch: the guide previously reserved this removal for a
  versioned boundary, and this is it. (#39)

- **A focused map no longer spends its budget on files the focus never
  reached.** A seeded ranking teleports to the seeds alone, so a file no
  reference path connects to them earns nothing from the walk -- it holds
  only the residue of the uniform vector the power iteration starts from,
  which renders as rank `0.0000` and is not zero. Those files were ranked
  into the answer and their symbols competed for and won the token budget.
  Measured on this repository at `--focus contrib/hooks/agentless_gate_mark.py
  --max-files 10`: 13330 characters before, 3585 after, with the same ten
  files listed both times.

  The files stay listed, with the true count of what each holds. Dropping
  them would be the bounded-view-mistaken-for-complete failure, and the tool
  description publishes a top-N of ten however large the repository is. What
  changed is where the budget goes. `symbols_available` in the JSON now
  counts what competed for the budget, because the banner offers "raise the
  budget for the rest" and no budget renders a symbol in an unreached file.
  An unfocused map teleports uniformly, reaches everything, and is
  byte-identical to 0.6.7. (#38)

- **The stale-cache receipt names the repository to reindex.** It read
  `run agentless-mcp index for performance`. An agent following that hint
  from a shell whose working directory is not the repository indexes the
  wrong tree, and the natural guess `--root` is not the flag. It now reads
  `run agentless-mcp index --repo <path> for performance`, matching the
  wording the empty-cache hint already used, with the path shell-quoted. (#38)

### Added

- **`PageRank.support`.** The set of files the walk can reach from the
  teleport vector's support, over the same augmented adjacency the iteration
  steps along -- backflow edges included, so a file that references the seed
  counts as reached. This is the invariant behind the map change. A threshold
  on the rank itself would not work: measured 2026-08-25, a twenty-node
  component disconnected from the seed converged at 1.2e-7 per node, which
  renders as `0.0000` and passes `> 0.0`.

- **A test pinning the gate hook's operation set to the server's tables.**
  `contrib/hooks/agentless_gate_mark.py` hand-mirrors the v2 operations that
  unlock broad search. The hooks fail open, so drift was silent: a renamed
  operation would leave the hook unlocking on a spelling the server no longer
  serves, and the only symptom is an unexplained early `Grep` denial in a
  session that did localize. The test also asserts that the six deliberately
  excluded operations still exist, so a removal upstream is not mistaken for
  the exclusion. (#37)

### Not changed

- **The gate still unlocks once per session.** Filed as item 3 of #38 and
  labelled there as a design observation rather than a bug. Both directions
  it suggests -- expiring the marker, re-locking on a repository change --
  add complexity, and the issue asks for a paired run showing the need before
  either ships. No such measurement exists yet.

## 0.6.7 -- 2026-08-25

Codex CLI runs the same gate.

### Fixed

- **Codex's unified-exec wrapper is gated.** Codex reports the canonical name
  `Bash` for ordinary shell calls, which 0.6.6 already covers, but names its
  unified-exec tool `exec_command`. That name fell through the hook as "not a
  search tool", so a client with unified exec enabled routed every search past
  the gate. `SHELL_TOOLS` now holds both names.

### Added

- **Codex CLI install instructions.** The two hook scripts are unchanged
  between clients: Codex uses the same `PreToolUse`/`PostToolUse` events, the
  same `session_id` / `tool_name` / `tool_input` payload fields, and the same
  exit-2-blocks-with-stderr contract. Three things differ and all three are
  configuration: the MCP server entry must be named `agentless` because the
  server name is the tool-name prefix the mark hook matches, the recommended
  matcher gains `exec_command`, and Codex requires the hooks to be reviewed
  once with `/hooks` before it will run them.

### Known gaps

- **Codex defers MCP tool schemas and offers no eager-loading control.** The
  `anthropic/alwaysLoad` hint the server sets is Anthropic-specific and absent
  from Codex; its own `tool_search_always_defer_mcp_tools` is fixed on. Codex
  therefore runs the deferred-plus-gate configuration, which 0.6.2 measured as
  indistinguishable from eager on every metric, rather than the eager one
  0.6.3 chose.
- **`exec_command` gating is untested against a live unified-exec session.**
  The name comes from Codex's hook documentation and is covered by a unit
  test; no run with that feature enabled has exercised it.

## 0.6.6 -- 2026-08-25

The gate covers search routed through the shell. Closes #33.

### Fixed

- **A `Bash` command that searches the tree is gated like a `Grep`.** The
  PreToolUse matcher keyed on the tool names `Grep|Glob`, but a harness that
  instructs the model to search with `grep`, `rg` or `find` as shell strings
  produces payloads whose tool name is always `Bash`. Those never reached the
  check hook, so in that mode the deny half of the gate was dormant and the
  ordering constraint failed open. This is the gate's own rule -- condition on
  the invariant, not a proxy -- applied to the gate itself: the invariant is
  "no broad text search before localization" and two tool names were the
  proxy. The recommended matcher is now `Grep|Glob|Bash`.
- **The heuristic denies only what parses cleanly as a tree search.** `rg`
  with no path operand or a directory operand, `grep` with a recursive flag
  (including a bundled cluster such as `-rn`), and `find` with
  `-name`/`-path`/`-regex`. Everything else passes: a pipe filter over another
  command's output, `grep` reading stdin, a search scoped to one existing
  file, a command that cannot be parsed, and any command that is not a search. Most
  `grep` in a shell session filters console output and touches no repository
  file, so that is the false-positive budget the rule is built around.
- **One marker still governs both routes.** A structural call unlocks the
  shell path and the native path together, and after unlock no command is
  parsed at all.

### Known gaps

- **This half of the gate is a weaker guard and fails open by design.** Shell
  text cannot be parsed in general. `git grep`, a search assembled through
  `xargs` or a subshell, and any command whose name arrives through a variable
  all pass unexamined. Denying on doubt would spend the gate's credibility on
  calls that touch no repository file, so doubt reads as allow.
- **The shell heuristic is unmeasured.** The paired benchmark behind the
  "recommended install" claim covered the tool-name matchers. This rule has a
  different error profile, and a Bash-first arm should be measured before the
  claim is extended to it.

## 0.6.5 -- 2026-08-25

The gate's two halves now agree on what counts as already localized, and every
decision logs the operation it acted on.

### Fixed

- **`read(slice)` unlocks broad search.** The check hook already lets an
  exact-file `Grep` through before any unlock, on the grounds that the caller
  has localized that search itself. `read(slice)` names a file *and* a line
  range, which is stronger evidence of the same thing, yet 0.6.4 refused it as
  a "raw read". The two halves of one gate disagreed about the same rule. The
  v1 spelling `read_slice` unlocks with it. `read(dir)` stays out on the
  distinction that does hold: a directory listing is how you look for a file,
  not evidence that you found one.
- **The gate log records the `operation`, not only the tool.** Now that
  `read(slice)` and `read(dir)` are treated differently, a log naming only
  `read` could not say afterwards which one a session was refused. A benchmark
  run against 0.6.4 hit exactly that: 32 refused `read` calls that could not be
  split. Both `structural_call` and `agentless_call_ignored` records carry the
  field.

### Measured

- **The 0.6.4 gate narrowing has no detectable effect on retrieval, and the
  method that would have reported one is unreliable at this sample size.** A
  paired 60-instance run on SWE-Explore-Bench, both arms on the same build,
  differing only in the unlock rule, returned three significant losses out of
  seventeen metrics. Splitting the instances by whether the gate ever fired
  dissolves them: the gate fired in 12 of 60, and `hit_file_rate`'s loss is
  *larger* in the 48 instances it never touched (-0.068) than overall (-0.054).
  In those 48 the two arms were the same configuration, so that figure is the
  benchmark's noise floor rather than an effect. Counting which arm each metric
  favours makes the point plainly: 16 of 17 favour the old gate overall, and 13
  of 17 still favour it in the subset where nothing was ever denied.
- **Consequence for earlier releases.** The paired bootstrap resamples
  instances and treats each score as fixed, so it never modelled the agent's
  run-to-run variance. Effects at or below roughly 0.05 on these metrics are
  not resolvable at n=60 without a same-arm replicate, which includes the
  gate's own founding result against the grep baseline (+0.052
  `weighted_core_coverage`). Those results are not withdrawn; they are
  unreplicated, and future arms should quote a measured noise floor beside the
  effect.

### Kept

- **The gate still fires on the rule 0.6.4 introduced.** `capabilities`,
  `symbols(locate)` and the shape listings do not unlock broad search, and the
  marker is still a digest of the session id. Nothing in the measurement
  disconfirmed the narrowing; it closed a real hole and a real path-escape, and
  it is kept on those grounds rather than on a retrieval claim.

## 0.6.4 -- 2026-08-25

`validate` runs one command batch per distinct result rather than per
candidate, and the structural-first gate distinguishes localizing calls from
diagnostics.

### Changed

- **`validate` groups candidates by their exact resulting file state and runs
  each group once.** Every candidate is normalized against unpatched HEAD
  before any command is scheduled; candidates whose changed paths and contents
  are byte-identical share one worktree and one command batch, while each
  keeps its own id, index and vote multiplicity. Sampling the same fix twice
  used to cost two full suite runs. Grouping keys on the exact tree delta and
  never on the AST equivalence key: two candidates that are AST-equivalent but
  source-different still execute separately, because only byte-identical
  content proves the two worktrees are the same.
- **A candidate whose regression command fails no longer spends a reproduction
  command.** The vote's reproduction rung requires `regression_passed and
  reproduction_passed`, and the regression rung ignores reproduction
  entirely, so the second command cannot change that candidate's rank. Its
  `reproduction` is reported `not_evaluated` rather than guessed.
- **Verdict records carry `execution_group` and `executed_as`.** Both fields
  are additive: `load_verdicts` reads named fields, so an older reader is
  unaffected. `executed_as` names the representative whose command evidence a
  record reuses, and output tails ride with that representative rather than
  being copied onto every member.
- **A candidate that cannot be parsed or normalized costs no worktree.** The
  refusal now happens in the planning pass, not inside a worktree created for
  it.
- **The structural-first gate keys on localization, not on any Agentless
  call.** `orient(map|path)`, `symbols(find|overview|expand|explain)` and
  `find_referencing_symbols` unlock broad search. Diagnostics, raw reads,
  `symbols(locate)` and the shape listings
  `orient(communities|cycles|diagram|health)` do not, so `capabilities` can no
  longer unlock `Grep`. A `Grep` scoped to one existing file is allowed before
  unlock, because the caller has already localized that search.

### Fixed

- **An untrusted `session_id` no longer becomes a filesystem path component.**
  The gate marker is now `/tmp/agentless_gate/<sha256(session_id)>.json`
  carrying an unlock receipt, where it was `<session_id>.ok`. A session id
  containing path separators previously escaped the marker directory.
- **A verdict that did not apply no longer carries an equivalence key.** The
  planning pass computes a key before the run, and projecting it onto a
  representative's `failed` or `not_evaluated` record made the receipt state
  two incompatible things about one candidate. The vote excluded those
  candidates on `applied` and `measured` first, so no ranking was affected.
- **The check hook logs a tool it does not govern as `not_a_search_tool`.** It
  read as `malformed_payload`, which named a broken `Grep` rather than a hook
  matched more widely than the gate.
- **The gate fails open when `/tmp` cannot hold session state.** The check
  hook proves it can write a marker before it enforces one's absence;
  otherwise a read-only `/tmp` denied every search for the whole session.

### Kept

- **Both hooks still fail open.** Malformed stdin, a payload with no session
  id, or any internal error exits 0 and allows the call. The mark hook reads
  the call and not its result: `tool_response` has no shape it can read a
  success out of across every tool it matches, so an Agentless call that
  errored still unlocks.
- **`--jobs` still produces the same verdicts document at any value.** Output
  order is sorted by candidate index, not by completion.

## 0.6.3 -- 2026-08-25

One ranking fix and one reversal: the map answers with relatives of the seed
instead of the repository's hubs, and the five MCP tools ask for eager schema
loading again.

### Fixed

- **The ranking walk steps both ways, so a seeded map stops padding rank two
  onward with high-centrality noise.** Reference edges run referrer to
  definer, so a walk that only followed them left every seed heading downhill
  into whatever that file imports -- the utility modules every file imports
  and which therefore rank high under any personalization. `map --focus
  UnicodeUsernameValidator` on Django answered with the class's own file at
  rank one and `django/utils/translation/__init__.py`, 30-plus irrelevant
  symbols of it, at rank two. The walk now also steps referrer-wards at half
  weight (`BACKFLOW_WEIGHT`), which is what makes the files that *use* a seed
  reachable from it: before, a file referencing the seed scored exactly zero,
  the same as a file with no connection to it at all. The graph itself keeps
  its direction -- `flood` and `reverse_adjacency` are unchanged; only the
  ranking walk reads it as undirected.
- **A resolved seed leads the ranked list.** It is the file the caller named,
  and the walk is the map's guess about what else is relevant, so direct
  evidence opens the answer. This was already wrong before the change above:
  on 5 of 24 seeded Loc-Bench instances the walk gave rank one to some other
  file, which reads as the map ignoring the focus.
- **Test files are held out of the ranking rather than merely absent from
  it.** The test companion section exists because a test was a pure source of
  rank, which backflow would have undone -- a suite references many files, and
  a leaf they reference has nowhere else to send its rank, so tests would have
  ranked above their subjects. `personalized_pagerank` now takes the pure
  sources explicitly and the map passes its test paths. Measured: without the
  rule, test files took 30 of 250 top-5 slots on the Loc-Bench subset, against
  9 before the change; with it, 7.
- **Measured on 50 Loc-Bench V1 instances, deterministic and model-free.**
  Seeded: `acc_any@5` 0.540 to 0.660, `nDCG@10` 0.370 to 0.430, MAP 0.295 to
  0.374. Unseeded: `acc_any@5` 0.340 to 0.480, `nDCG@10` 0.194 to 0.253, MAP
  0.136 to 0.199. Paired over instances with a 10,000-sample bootstrap, MAP
  gains +0.079 (95% CI +0.029 to +0.132, 29 instances better and 10 worse) and
  `nDCG@10` +0.060 (95% CI +0.009 to +0.112). `recall@10` moves +0.006 (95% CI
  -0.071 to +0.084), so this buys precision at the top of the list and costs
  no coverage -- unlike the span-cropping arms tried earlier, which bought
  precision by losing it. Two consecutive runs differed on 0 of 50 instances.

### Changed

- **`anthropic/alwaysLoad` is restored on the five v2 tools.** 0.6.2 removed
  it on the reasoning that the structural-first gate's own denial loads each
  schema on demand, so eager loading charged every session a context cost for
  adoption the gate already guaranteed. The premise was not measured, and it
  is wrong. Reading the harness token accounting for the two arms -- same 60
  tasks, same gate, same prompt, differing only in schema policy -- deferral
  cost *more* on every measure: cache-creation tokens +2,563, cache-read
  tokens +19,014, turns +0.67, output tokens +303, wall +4.9s, all paired
  per-instance. No interval excludes zero, so the honest claim is that
  deferral bought no measurable saving, not that it was significantly worse.
  The mechanism explains the direction: deferring does not keep the schemas
  out of context, it makes the agent spend a round trip fetching them first.
- **The quality metrics were indistinguishable, and the one exception does not
  track schema policy.** Across the same paired comparison, every metric sat
  inside its interval except `recall@100`, where deferral led by 0.017 -- one
  significant result among roughly 22 tests at n=60. It reads as noise rather
  than an effect, and the arm ranking says the same thing: the tools-only arm
  is eager-loaded and holds the highest `recall@100` of any arm (0.1351),
  which a context-pressure story cannot produce.
- **What eager loading buys is state at the moment the gate fires.** Loading a
  schema because a call was blocked makes the tool callable; holding it from
  the first turn makes the tool considered, by an agent that knows what it
  answers before it picks a first move. That is a reasoning argument, not a
  measured one, and it is the tie-breaker rather than the case: the case is
  that deferral cost a round trip and returned nothing.
- **The shipped prose matches the server again.** The README section is now
  "Eager tool schemas" and states the cost as well as the reason; the packaged
  agent guide and the skill say the tools ask to be loaded eagerly. The gate
  remains the load-bearing mechanism in both policies, and the sections that
  describe it are unchanged.

## 0.6.2 -- 2026-08-24

A packaging-level change to how the MCP tools reach an agent. Nothing inside
the tools behaves differently.

### Changed

- **The five MCP tools no longer request eager schema loading.** 0.5.0
  published `anthropic/alwaysLoad` on `orient`, `symbols`,
  `find_referencing_symbols`, `read` and `capabilities`, so that a client which
  defers tool schemas behind a search step -- Claude Code tool search -- held
  all five in context from the first turn. The tools now publish no such hint,
  and a deferring client loads each schema when a session first needs it. The
  reason is the structural-first gate in `contrib/hooks/`: it denies `Grep` and
  `Glob` until the session has made one `mcp__agentless__` call, so an agent's
  first native search is redirected to these tools and the client loads the
  deferred schema at that moment. Eager loading paid a per-session context cost
  for the adoption the gate now guarantees.
- **This reverses the 0.5.0 `alwaysLoad` decision deliberately, on
  measurement.** In a paired comparison on SWE-Explore-Bench against a healthy
  server -- n=60 issue-localization tasks on Sonnet -- an arm restricted to the
  agentless tools beat a free-choice arm head to head on precision (+0.062),
  recall (+0.041) and F1 (+0.040), with every 95% confidence interval excluding
  0. What moved those numbers was the ordering, which the gate enforces, not
  the schema budget the hint spent.
- **The shipped configuration was measured as its own arm before release.**
  The gate against this release's deferred schemas, on the same 60 tasks,
  matches the eager-schema gated arm within every confidence interval except
  `recall@100`, which moves +0.017 in the deferred arm's favour -- one
  significant result among roughly 22 tests, so read it as no cost and
  possibly a small benefit. Full numbers and the arm definitions are in
  `docs/analysis/benchmark-methodology.md`.
- **Without the gate installed, the deferred schemas make native search the
  path of least resistance again.** A deferred tool is not a callable tool, and
  `Grep` loads from the first turn. Install the gate. The README section
  "Structural-first gate" is the recommended setup, and this release assumes an
  ordering mechanism of that kind exists client-side.

### Fixed

- **The test suite scrubs ambient `GIT_*` variables before any test runs.**
  Git exports `GIT_DIR` to hook processes, and `GIT_DIR` overrides the `-C` a
  fixture git call is given: a suite run from inside a pre-push hook
  reinitialized the enclosing repository as bare and rewrote its index with
  fixture files. The package's own git calls already scrubbed the family
  (`core.gitinfo.subprocess_env`); `tests/conftest.py` now applies the same
  scrub once at import.

### Kept

- **The five-tool surface, including `capabilities`.** It is the diagnostic for
  the degraded path: agents never reached for it against a healthy server (0 of
  60 tasks) and called it in 46 of 60 tasks against a server that answered
  about empty repositories, which makes its usage rate a distress signal worth
  publishing. Deferred, it costs a session nothing until something goes wrong.

## 0.6.1 -- 2026-08-24

A correctness release with one fix, and a correction to what 0.6.0 claimed.

### Fixed

- **A root an enclosing repository ignores is no longer served as empty.**
  `walk_repo` took the git branch whenever git answered for the root, which is
  a correlate rather than the invariant. When the served root sits inside an
  enclosing work tree that ignores it wholesale -- a snapshot unpacked under a
  gitignored `repos/` directory, carrying no `.git` of its own -- git answers,
  `git ls-files --cached --others --exclude-standard` returns zero paths, and
  every tree view, directory listing and map built on the walk reported an
  empty repository for a tree full of files. The branch now keys on ownership:
  git's listing is used when the answering repository's top level is the root
  itself, or when `git check-ignore` says that repository does not exclude the
  root. A disowned root takes the bounded walk, exactly like a root outside any
  repository. When `check-ignore` cannot answer -- a timeout, a git that cannot
  be run, git's fatal exit 128 -- the git listing is kept, so the fix cannot
  widen into a fallback for roots that list correctly today.
- **`core.treewalk.is_git_repo` is gone,** replaced by the private
  `_git_listing_speaks_for`. It had one caller, the one above, and a predicate
  that answers "does git answer for this path" is the defect waiting to be
  reintroduced by the next caller that reaches for it.

### Correction to the 0.6.0 entry

The 0.6.0 entry below is kept as published. Its benchmark validation is not.

- **0.6.0's benchmark validation was invalid, and the defect above is why.**
  The SWE-Explore-Bench harness unpacks each instance's snapshot repository
  into a gitignored directory inside the bench work tree, so every repository
  under test was a disowned root. The server listed zero files for all of them,
  in the 0.5.0 run and in the 0.6.0 run alike. Both runs therefore measured the
  server answering about empty repositories, not about the code they contained.
- **The "recovers test-file localization" claim is unvalidated,** pending a
  re-run of the benchmark against 0.6.1. Nothing in 0.6.0 is withdrawn on the
  strength of those numbers either; they say nothing in either direction.
- **The mechanical `is_test_path` widening stands.** It was measured against
  the ground-truth file lists rather than through the server: 11 more
  ground-truth files are recognised as tests, and none that were recognised
  before are lost. That measurement does not touch the walk.

## 0.6.0 -- 2026-08-24

Recovers test-file localization and adds a structural-health view. The
benchmark regression this release exists for was algorithmic, not a tuning
mistake: `build_graph` writes edges referrer to definer, so a test file is a
pure source with no inbound weight, and a ranking that scores inbound weight
could never place one however directly it exercised the code above it.

### New

- **`orient(map)` lists the tests that exercise the files it ranked.** A
  backward flood over the reference graph finds them and reports each as one
  row with a real `path:start-end` span, outside the token budget the ranked
  files are packed into. The section is omitted entirely when it is empty.
- **`orient(health)` and `agentless-mcp health`.** Orphan candidates, unused
  exports and hubs over the resolved graph, gated on same-file and imported
  evidence tiers with the discounted tiers named per row.
- **`core.graph.flood`.** A depth-bounded breadth-first walk, forward or
  backward, that reports how far each reachable file is and whether it
  finished looking.
- **`relation_weights` project-config key.** Weights edges by relationship
  kind. Off by default, and **not language-neutral**: only the Python class
  handler fills in the base classes it reads.
- **Community identity.** Each community carries `member_hash`, equal exactly
  when the member set is equal, so a group can be matched across two runs.
- **`SyntaxVerdict.first_error_line`.** A failing verdict names the line of
  the new content's first parse error instead of only a count.

### Behaviour changes

Read this section before upgrading a script or an agent loop.

- **`is_test_path` replaces `_defined_in_tests` and is public.** It now
  recognises in-package test names -- `config/os_test.go`,
  `shareUrl.test.ts` -- which the directory rule alone could not see. Note
  that a repository keeping OpenAPI or protocol documents under `spec/` has
  those files read as tests, which excludes them from `health` and ranks them
  below production among shared-caller candidates.
- **Reference sites truncate round-robin rather than by a flat prefix cut.**
  The old cut was alphabetical, so `src` sorted before `tests` and the dropped
  tail was disproportionately test files.
- **The map's JSON gains a `test_companions` key** carrying `total`, `limit`,
  `omitted`, `exhausted` and `rows`. Additive: no existing key changed.
- **The rendered companion row reads `-- file references`, not `covers`.**
  The span is one referencing symbol while the named files are the whole
  file's reach, and the old wording read as a promise that the cited lines
  mentioned every name beside them.
- **A capped companion walk says so.** `test_companions.exhausted` is true
  when the backward walk hit its node bound, and the rendered section adds a
  note. Without it a truncated reach set read as a complete one, which
  reports a test as absent that nobody looked for.

### Fixed

- **The companion walk no longer rebuilds its candidate set per test file.**
  The set depends only on the flood depth, so it is built once per depth.
  Measured on a synthetic graph of 8000 test files and 8000 helpers: 4.23s
  before, 0.019s after, and the cost is now linear rather than quadratic.
- **`community_hash` deduplicates its members** before hashing, so the digest
  is a function of the member set the docstring promises. No shipped caller
  passes duplicates, so no digest changes.
- **`base_name` moved to `core.symbols`,** the only home below both `graph`
  and `resolve` that does not close an import cycle.

## 0.5.1 -- 2026-08-23

A correctness and security release. No new features.

Every fix below was reproduced with a command before it was written and
re-verified by an independent reviewer afterwards. Where a fix changes what an
existing invocation does, it is listed under **Behaviour changes** rather than
folded into the fix list, because a patch version number is not a place to hide
one.

### Behaviour changes

Read this section before upgrading a script or an agent loop.

- **`expand` reports a partial batch as a failure.** It now exits 1 when any
  requested id fails to resolve; it previously exited 1 only when *none*
  resolved. A batch that answered 49 of 50 ids exited 0 before and exits 1 now.
  This matches `skeleton`, and it exists because a partial answer returned as
  success is the failure this release is largely about.
- **`expand` writes its failures to stderr.** They used to be rendered into
  stdout among the symbol bodies, so a caller parsing stdout received the
  failure report as if it were content.
- **Numeric ceilings are enforced on the command line.** `--limit` above 500,
  `--depth` above 20, `--max-entries` above 20000, `--context` above 200,
  `--max-nodes` above 500 and `--resolution` above 100 previously succeeded on
  the CLI while the MCP door refused them. Both doors now apply the same rule,
  which lives once in `util/bounds.py`. The numbers are the ones the MCP schema
  already published, so no previously-documented limit changed.
- **`map --budget` is bounded.** Values below 200 or above 64000 are refused.
  The configuration file and the MCP schema already enforced that range; the
  CLI flag did not.
- **`diagram --max-edges 0` is accepted over MCP.** The schema required at
  least 1. Zero reference edges is a legible diagram, and the CLI always
  allowed it.
- **`application.symbol_service.render_expansion` no longer returns the
  unresolved rows.** They moved to `unresolved_lines`, which callers rendering
  the text form themselves should now also call. Both shipped adapters were
  updated.

### Security

Every item here is reachable from a repository someone else wrote.

- **A filename can no longer forge the tool's own framing.** The receipt's
  summary line was interpolated unescaped, so a path containing a newline
  produced a second `#`-prefixed line inside the region an agent is told to
  trust -- above the untrusted-content banner, where it can carry directive
  prose. Reached through `diagram --focus`.
- **A filename can no longer forge a structural row.** Four renderer sinks
  emitted a service-built message unescaped, so a path embedded in a stable id
  could end the line early and have the remainder read as tool output. Reached
  through `path` and `diagram`.
- **Scratch worktrees stay out of the repository under analysis.** The
  containment guard compared an unresolved cache path against a resolved
  repository root, so pointing `XDG_CACHE_HOME` at a symlink into the
  repository walked through it and created worktrees inside the tree being
  analysed.
- **A manifest symlinked out of the repository is refused, and says so.**
  `lint` read `pyproject.toml` and `requirements*.txt` without a containment
  check, so a hostile repository could have its declared dependencies read from
  any file the user could read. The refusal is reported as a warning rather
  than a silent empty result, so an empty declared set is not mistaken for
  "this project declares nothing".
- **Escaped output is encodable.** A filename that is not valid UTF-8 reached
  the renderer as a lone surrogate and raised at encode time. `U+2028` was also
  escaped as `\x2028`, which reads back as a space followed by `28`.
- **The validation guard sees around path spellings.** `./` and `../` forms of
  a protected test-configuration path walked past the prefix test that the
  canonical spelling was caught by.

### Correctness

- **A patch naming a non-ASCII path no longer edits the wrong file.** The path
  header test was ASCII-only, so `src/naïve.py`, `src/café/app.py` and any
  other non-ASCII path was read as prose and its block silently inherited the
  previous block's path -- an edit applied to a file nobody named, reported as
  clean.
- **A truncated diff section is refused rather than noted.** A section carrying
  hunks but no `---`/`+++` pair reported "mode change only, so there is no
  content to check". A hunk whose body outran its declared counts and whose
  first excess line was blank or `--` ended the hunk there and silently skipped
  every later hunk in that file.
- **The patch linter stops going blind after a nested format spec.** A doubled
  brace was treated as an escape at every depth, so `f"{n:>{w}}"` left the
  scanner inside a string for the rest of the fragment and every later check
  returned no finding rather than reporting a gap.
- **The patch linter no longer misreads `lambda`.** An unbracketed lambda
  parameter list was split on its own commas, producing wrong argument counts
  in both directions. It now declines to judge, as it already did for star-args.
- **An unreadable cache is reported, not treated as an absent one.** A disk I/O
  error reading the indexed file list was answered as "no usable index", which
  silently re-parsed the repository and reported the cache as fresh.
- **A schema migration is one transaction.** The version bump dropped and
  recreated tables as independent statements, so an interrupted upgrade could
  leave a database that no later run would repair.
- **`index` refuses instead of crashing.** A full disk or an unwritable cache
  directory ended the command in a raw traceback.
- **A distinct module no longer shares a diagram label.** Two paths differing
  only outside the mermaid label allowlist rendered as the same box.
- **Suppressed configuration warnings are counted honestly.** One oversized
  warning suppressed every smaller warning behind it, and the count line
  implied the shown warnings were the first ones.

### Performance

- **The Python reference pass is linear again.** A scope lookup walked a node's
  parent chain per identifier, so cost was proportional to identifiers times
  parse depth -- and a left-nested expression makes depth equal file size. A
  6 KB file holding one long chained expression took **7.64 s**; the same file
  takes **0.007 s**. A realistic 8.5 KB type union of 1200 members took
  **3.94 s** and takes **0.005 s**. With the 1 MB per-file cap, a single
  generated file could stall every scan of a repository. The resolved edge set
  is byte-identical across the change on a pinned 178-file, 20-language corpus.

### Extraction

- **C and C++ member definitions are symbols.** An out-of-line definition, a
  constructor, a destructor, an `operator==` and an in-class method were all
  absent: a routine `.cpp` yielded one symbol out of five function definitions.
- **Java, Kotlin, Scala, PHP and C# imports bind the name they import.** Those
  five recorded no bound name, so a call through an imported symbol resolved as
  a repository-wide name guess rather than as an import.
- **A visibility keyword beats the underscore convention.** A `private` method
  in TypeScript, TSX, JavaScript or Swift was reported public.
- **Package imports resolve in a `src/` layout.** A package named through its
  `__init__.py` matched no file, so any import cycle routed through one was
  invisible. Unresolved internal imports on this repository fell from 115 to 18.
- **Rationale comments keep their line numbers.** A form feed, vertical tab,
  `U+0085` or `U+2028` inside a block comment shifted every marker after it,
  because the split disagreed with the row grammar the parser used.
- **The built-in name set no longer moves with the interpreter.** It was read
  from the running `builtins`, so the same repository at the same commit
  classified names differently under 3.10 and 3.13 while the cache key stayed
  the same.

### Continuous integration

- **A failing test suite fails the build again.** The test step piped pytest
  through `tee` under a shell without `pipefail`, so the step reported success
  whatever pytest returned.
- **The skip ceiling counts skipped tests.** It counted summary lines, which
  pytest groups, so it measured distinct skip reasons instead.

### Upgrading

The cache schema is version 12. A database written by any earlier release,
including 0.5.0, is discarded and rebuilt on first use; no action is required
and no answer is served from stale rows. The rebuild is the only cost.

Every released build wrote schema 7, so the rebuild would have happened at any
version number above it. Version 12 rather than 11 exists for a narrower case:
the staleness check compares versions for inequality, so anyone who ran an
unreleased build of this work already holds a version 11 cache, and 11 would
not have invalidated it -- it would have gone on answering from rows the
pre-fix extractor wrote. If you tested this branch before release, this bump is
what clears that for you.
