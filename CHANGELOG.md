# Changelog

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
- **Without the gate installed, the deferred schemas make native search the
  path of least resistance again.** A deferred tool is not a callable tool, and
  `Grep` loads from the first turn. Install the gate. The README section
  "Structural-first gate" is the recommended setup, and this release assumes an
  ordering mechanism of that kind exists client-side.

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
