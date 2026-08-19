# Phase 7 — Adversarial Review

**Who produced which column.** The **my verdict** column is this reviewer (Opus 5),
reading the cited source directly and, where a claim was mechanically checkable,
running it. The **local coder** column is the **local Gemma assist backend**
(`gemma-4-12b-it-UD-Q6_K_XL`, served by `mcp__docs__consult` on the local assist
port) — a small model given the real code with the claim phrased adversarially and
asked to refute it. Gemini was not used and was not invoked. The local model is
recorded as a **second opinion only**; my own reading of the code is the tiebreaker,
and the one disagreement is called out explicitly below.

Everything marked "measured" in this document was run against the working tree at
`bf7b21b` during this pass; the commands are named inline. The suite state at the
time of review: `python -m pytest -q` → **1141 passed, 4 skipped in 16.28s**.

---

## Verification Table

| # | Claim | My verdict | Local coder verdict | Note |
| --- | --- | --- | --- | --- |
| a | `server.py:187` unions client-advertised MCP roots into the authorization allowlist, so `--root` confines nothing | **CONFIRMED** (severity challenged) | CONFIRMED | `allowed = list(dict.fromkeys([*self._roots, *client_roots]))` (`server.py:187`) is the only allowlist `resolve_repo` ever sees, and `_authorise` (`repo_context.py:96-108`) tests exact membership in it. Measured extra: `_sole_selection` (`server.py:116-117`) returns `client_roots[0]` when the advertised root matches *no* configured root — `_sole_selection([/a,/b,/evil], [/a,/b], [/evil])` → `/evil`. So an omitted `repo_root` resolves to a directory the operator never configured. See Severity Challenges. |
| b | `cache.py:320-368` — a stale `CachedSource` snapshot returns `[]` symbols/refs with a `fresh` receipt | **CONFIRMED** (severity challenged) | CONFIRMED | `_fresh_digest` (`cache.py:327`) reads `self._state.entries`, snapshotted once at `open_source` (`cache.py:471,488`); `_symbol_rows`/`_ref_rows` (`cache.py:341,360`) run later in separate autocommit statements. The prune that empties them is real: `_apply_plan` (`cache.py:694,703-706`) deletes every `previous` path not in `plan.seen`, and `_plan_index` (`cache.py:646-648,661-664`) omits from `seen` any file that failed `read_bounded` or whose grammar was cold. Nothing distinguishes "pruned" from "legitimately defines nothing". |
| c | `envelope.py:127/136` — receipt header excluded from the token budget + uncapped repo-controlled config warnings | **PARTIAL on the wording, CONFIRMED on the defect** | CONFIRMED | The header *is* subtracted (`budget = max_tokens - counter.count(header)`, `envelope.py:136`); it is never **capped**. `_fit` (`envelope.py:206-221`) only ever shrinks `body`, and on `budget <= 0` returns `("", all lines)` — so `wrap` emits the header whole and the answer not at all. Measured this pass with `Chars4Counter`: a 52,000-byte `.agentless-mcp.json` of unknown keys (under the 65,536-byte cap at `projectconfig.py:43`) → 4,000 warnings (`projectconfig.py:201-205`) → **152,065 tokens / 608,263 chars from a `max_tokens=16000` call, with the body entirely dropped**. `wrap_json` on the same context: **145,161 tokens**. The panel's 301,789 figure is the same defect at higher key density. |
| d | `sandbox.py` `git worktree add` runs the analysed repo's own git hooks on the default patch-apply path | **CONFIRMED (measured)** | **REFUTED token, CONFIRMED body — see disagreement** | Measured: a repo with `.git/hooks/post-checkout` → `git worktree add --detach ../wt HEAD` **executed the hook**, cwd inside the new worktree, and created `.git/worktrees/wt` inside the analysed repository. `sandbox.run_git` (`sandbox.py:392`) passes no `-c core.hooksPath=…`, no `--no-checkout`, no `GIT_CONFIG_NOSYSTEM`. Amplifier the panel did not state: `validate_service.py:452` opens a worktree **per candidate**, so a 50-candidate run fires the hook 51 times. |
| e | `validate`'s `test_cmd` falls back to the analysed repo's `.agentless-mcp.json` | **CONFIRMED** (severity challenged) | CONFIRMED | `main.py:1004` — `test_cmd = args.test_cmd if args.test_cmd is not None else ctx.config.test_cmd` — against `validate_service.py:29-33`'s *"Nothing here reads a test command out of the repository under analysis … letting it nominate its own judge is the injection path this whole package is shaped to avoid."* The guards at `projectconfig.py:311-334` and `sandbox.py:236` constrain the command's *shape* (single line, length cap, argv not shell); none of them constrain which program runs. The note at `main.py:1013-1017` is informational, not a gate. |
| f | `patch_service.py:339-343` in-place apply writes back lossily-decoded text, corrupting non-UTF-8 bytes with `ok=True` | **CONFIRMED (measured)** | CONFIRMED | `read_bounded` (`fslimits.py:105`) decodes `errors="replace"`; `_apply_at` (`patch_service.py:343`) writes it back `encoding="utf-8"`. Measured: `# caf\xe9 …` → `# caf\xef\xbf\xbd …` on a line no edit touched. Second corruption neither the panel nor the claim states — see Independent Findings #1. |
| g | `treewalk.py:79-104` — `walk_repo`'s git path follows tracked symlinks outside the repo root | **CONFIRMED (measured)** | CONFIRMED | The git branch (`treewalk.py:79-80`) takes `git ls-files` output and tests only `candidate.is_file()` (`treewalk.py:93`), which follows the link; the non-git branch's containment check `_file_stays_inside` (`fslimits.py:195-203`) is never applied to it. Measured: a repo with `leaked.py -> /etc/hostname` committed → `walk_repo` lists `leaked.py` and `read_bounded(root/'leaked.py')` returns the **target's** content. `refs.scan_repo` (`refs.py:115-120`) and `cache._plan_index` (`cache.py:640-645`) both read this way, so `repo_map`, `find_symbol`, `find_referencing_symbols` and `index` all surface it. |
| h | `extractor.py` four per-child recursive walks; `RecursionError` at 248 chained JS calls aborts the whole-repo index | **CONFIRMED (reproduced exactly)** | CONFIRMED | `_visit_generic_children` ↔ `_visit_generic_node` (`extractor.py:790-791`, `:854`) are mutually recursive, against the module's own *"Iterative rather than recursive"* at `extractor.py:481-486`. Measured binary probe on `a.m0().m1()…`: 200 → ok, **248 → RecursionError**, matching the block reviewer's number to the call. `cache._plan_index:661` catches only `LanguageUnavailable`, so it propagates and kills the whole index run. |
| i | `find_referencing_symbols` asserts a total it recomputed from the truncated rows; `limit=0` returns a confident "no references" | **CONFIRMED** | CONFIRMED | `symbol_service.py:246` slices `sites[:limit]` while `RefsResult.total = len(sites)` stays honest; `render.py:788-789` then does `total = sum(len(group.sites) for group in groups)` and prints `f"{total} references to {target}"` from the truncated tuple. Text is the only form MCP returns. `limit` reaches this from the wire with no bound (`server.py:583`). |
| j | Unbounded numeric wire parameters: `read_slice(context_lines=-50)` / `lines=[[60,30]]` renders the whole file | **CONFIRMED (measured)**, cost claim overstated | CONFIRMED | Measured on a 201-line file: `line: 100` with `context_lines=10` → 23 lines; `context_lines=-50` → interval `(150, 50)` → **all 201 lines**; `context_lines=-100000` → `(100100, -99900)` → all 201 lines; `lines=[[60,30]]` → all 201 lines. Neither `resolve_locs` (`locs.py`) nor `slices._clamp` (`slices.py:97`) rejects an inverted interval, and `server.py:553` declares `context_lines: int` with no `Field(ge=0)`. |

### The one disagreement

On claim (d) the local coder wrote **REFUTED** as its verdict token and then, in the
same response, filed *"[Critical] Arbitrary Code Execution via Hooks — Git will
automatically execute any scripts found in `.git/hooks/` (e.g. `post-checkout`)
during the `worktree add` operation"* and *"[Major] Write Violation … `git worktree
add` writes metadata to the `.git/worktrees/` directory inside the analyzed
repository."* Its body agrees with the claim on both halves; only the token
disagrees, and the token appears to be answering "is the docstring fixable?" rather
than "is the claim wrong?". The code supports the claim, and I measured both halves
directly (hook executed, `.git/worktrees/wt` created). **CONFIRMED.** This is the
clearest illustration in this pass of why the local model's verdict is recorded as a
second opinion and not as evidence.

---

## Independent Findings (panel misses)

The block reviews are unusually complete. I checked eleven candidate misses against
`.audit/findings/` and `.audit/panel/`; eight were already filed (`is_git_repo`
subdirectory fallback B04-H1; `list_roots` untimed B22-H3; `wrap_json` oversize
B16-H2/B16:56; `_sole_selection` zero-candidate fallback B22-M4; comment-only
patches sharing the universal empty key B13:84; transient-failure pruning B08-M3;
`mcp_main`'s `ImportError` misdiagnosis B24-H1; `worktree prune` on the common path
B14/panel-reliability:219). Four survive.

**1. `patch_service.py:343` — every patch apply rewrites the whole file's line
endings on Windows.** `Path.write_text(content, encoding="utf-8")` is called with
the default `newline=None`, which enables universal-newline *translation on write*:
every `"\n"` in the buffer becomes `os.linesep`. `read_bounded` (`fslimits.py:101-105`)
reads bytes and decodes, so an existing `\r\n` survives into the string as `\r\n`,
and the write then turns it into `\r\r\n`. On an LF file the whole file silently
becomes CRLF. This is a second, independent corruption from the UTF-8 one in claim
(f), it affects **every** file the patch touches rather than only non-UTF-8 ones, and
on the `--in-place` path it lands in the user's own checkout. The repo supports
Windows deliberately (`sandbox.py:41-47`, `util/platforms.py`, the README Windows
section), and the existing CRLF findings are about a different code path —
B13-M1 is about *parsing* CRLF patch text and B12-L6 about the skeletonizer, neither
about the write-back. Fix: `newline=""` (one keyword).

**2. `util/filelock.py:83-86` — only `BlockingIOError` becomes `LockUnavailableError`.**
The POSIX `_acquire` converts `BlockingIOError` and nothing else. `fcntl.flock`
raises plain `OSError` for `ENOLCK` / `EOPNOTSUPP` / `EINVAL`, which is the normal
answer on NFS without `lockd`, on several FUSE filesystems, and on some overlay
mounts — all plausible locations for `$XDG_CACHE_HOME`. `cache._write_lock`
(`cache.py:1046-1051`) catches only `filelock.LockUnavailableError`, so such an
`OSError` escapes `build_index` unclassified, past every `except` the adapters were
written against. The module docstring at `filelock.py:3-6` claims the primitive's
one property is that a contended run is *told*; on a lock-less filesystem it is
neither told nor served. Fix: catch `OSError` and let the message distinguish
"held" from "this filesystem cannot lock".

**3. `util/filelock.py:60` — the lock file is truncated before the lock is taken.**
`handle = path.open("w", …)` opens with `O_TRUNC` and only then does `_acquire` run.
The write happens outside any mutual exclusion, by a process that has not yet been
granted the lock. Harmless today only because the protocol stores nothing in the
file — which is exactly the property a future maintainer adding a PID or a timestamp
to the lock file would not know they were depending on. The docstring at
`filelock.py:56-58` explains why the file is never *unlinked* and says nothing about
why it is truncated. Fix: `open("a")`.

**4. The `git worktree add` hook execution (claim d) is per-candidate, not
per-run.** `validate_service.py:452` opens `with sandbox.worktree(ctx.root)` inside
`_evaluate`, which `_validate_all` calls once per candidate (`validate_service.py:434-438`,
under a `ThreadPoolExecutor` at `jobs>1`). B14 and B21 both treat the worktree as a
single event. A `validate --candidates dir/` over 50 candidates executes the
analysed repository's `post-checkout` hook **51 times** (baseline plus each
candidate) and writes 51 `.git/worktrees/<id>` records into it. This changes the
finding from "one unexpected hook run" to "a loop that repeatedly executes
repository-configured code", and it is what makes the one-line `core.hooksPath` fix
worth doing immediately rather than scheduling.

### Checked and cleared (reported as a non-finding)

`grammars.get_parser` is `@cache`-memoized module-wide (`grammars.py:196-199`) and
`validate_service.py:435` drives `normalize`/`extract` from a `ThreadPoolExecutor`,
so all `jobs>1` threads share one tree-sitter `Parser` object. I stress-tested this
(8 threads × 30 parses of a 200-function file on one shared parser, `tree_sitter`
0.26.0): **zero mismatches, zero crashes**. The binding appears to hold the GIL
across `parse`. Recording it so the next reviewer does not re-open it on suspicion.

---

## Severity Challenges

**Downgrade (a) `--root` union: CRITICAL → HIGH.** The union is real and the three
docstrings that deny it (`repo_context.py:10-14`, `server.py:8-13`, `server.py:96`)
are false. But the adversary is the *connected client*, not the model and not the
analysed repository: `client_roots` arrives only from `context.list_roots()`
(`server.py:452`), there is no tool that lets a model set a root, and repository
content never reaches it. A prompt-injected agent cannot widen the allowlist. What
actually fails is an operator's belief that `--root /srv/app` is a confinement
boundary against a host whose workspace is `$HOME`. That is a serious mis-stated
guarantee and a design decision that must be made — it is not a remotely reachable
privilege escalation.

**Downgrade (b) cache TOCTOU: CRITICAL → HIGH.** The mechanism is confirmed and the
wrong answer is silent, which is the strongest part of the block reviewer's argument.
But the exposure window is one request, not a session: `ToolHandlers.resolve` calls
`_with_source` → `cache.open_source` on **every** tool call (`server.py:200-210`), so
the snapshot is at most milliseconds old when the rows are read, and the CLI is a
one-shot process. Realising it requires a concurrent `agentless-mcp index` **and**
that run committing a prune (i.e. a file that vanished, grew past 1 MB, or lost its
grammar) **and** the read landing in that window. Blast radius stays high; probability
does not support Critical.

**Downgrade (e) `validate` `test_cmd`: CRITICAL → MEDIUM.** The docstring is flatly
false and should be fixed. But the incremental security exposure is close to nil:
`validate` exists to run the analysed repository's test suite, and `--test-cmd pytest`
against a hostile repo already executes that repo's `conftest.py` on import. A
repository nominating the *argv* is not a meaningful escalation over a repository
controlling the *code that argv runs*. The finding's real content is (i) the false
guarantee at `validate_service.py:29-33` and (ii) `main.py:1013` being a printed note
rather than a gate. Rate it as the documentation/consent defect it is.

**Downgrade (d) worktree hooks: CRITICAL → HIGH.** Measured and real, but `git
clone` does not carry `.git/hooks` or `.git/config`, so this is **not** reachable
from cloned repository content — the usual "analyse an untrusted repo" story does not
reach it. It is reachable when a repository arrives as an archive with `.git` intact,
and it fires unconditionally on ordinary user-installed hooks (git-lfs installs a
`post-checkout`). Combined with Independent Finding #4 (51 executions per validate
run) and a one-flag fix, it belongs high on the list — on cheapness, not on severity.

**Uphold CRITICAL: (c) envelope, (g) symlink escape, (h) recursion.** (c) is
measured at 9.5× the ceiling it advertises with the answer dropped, from a file the
analysed repository controls, on every call. (g) is the only finding in the CRITICAL
set that is driven purely by *committed repository content* — symlinks survive
`git clone` — and it defeats the module that names itself "the security bound"
(`fslimits.py:1-14`). (h) takes the whole tool down on an input class present in
most JS/TS repositories, reproduced at exactly the stated threshold.

**Downgrade the cost half of (j): the funnel is not defeated.** B17-H1/B12-H2 call
`context_lines=-50` *"the exact token blow-out the funnel exists to prevent"*. It is
not: `envelope.wrap` still cuts the body at `DEFAULT_MAX_TOKENS` and appends a
`ceiling_truncation` marker (`envelope.py:136-143`) on this path, because here the
header is small. The cost is bounded at ~16k tokens. What is *not* bounded is the
honesty: the caller asked for 21 lines around line 100 and received the whole file,
truncated at an arbitrary point, with a marker that says the *ceiling* cut it rather
than that the request was inverted. Keep the finding, restate it as a wrong-answer
finding.

**Reject "large file ⇒ critical" for `patchlint.py`.** At 1,761 lines it is the
second-largest module in the repo and B15 produced no finding above Medium against
it. It is sectioned, its degradation set is explicitly enumerated
(`patchlint.py:178-188` — the tuple the cache scan should have copied), and its worst
finding is a docstring that forgot it reads `pyproject.toml`. Splitting it is not
work worth scheduling. **`extractor.py` at 2,090 lines is the opposite** — but its
severity comes from four hand-written recursive walkers with no shared traversal
(`:790`, `:854`, `:979`, `:1361`, `:1397`) against one iterative one (`:481`), not
from the line count, and the recursion is fixable without the split. Do not couple
the two: the split's stated prerequisite (moving `LANGUAGE_CONFIGS` and the node-type
tables out, B07-M6) makes it a multi-block change, and the outage does not need it.

---

## Top 5 Priorities

Ranked by blast radius × probability ÷ fix cost. Every entry names the smallest
change that removes the failure, not the largest change that would improve the file.

**1. Contain the extractor's `RecursionError` (h).** *Blast radius:* every read tool
on the repository, not one file. *Probability:* high and non-adversarial —
reproduced at 248 chained calls, a shape present in any minified bundle or generated
client. *Fix cost:* the containment is one line. Widen `cache._plan_index:661` to the
tuple `core/patchlint.py:178-188` already enumerates, so one pathological file becomes
one `IndexFailure` row. Converting the four walkers to `walk_nodes`' explicit-stack
form is the real fix and is a day; **do the `except` first, in its own commit**, so
the repo stops being one bundle away from unusable while the walkers are rewritten.
Add the 600-term regression test B07-C1 specifies.

**2. Cap the envelope header and move config warnings below the banner (c + B16-H3).**
*Blast radius:* every response from every tool, and the response is 9.5× the
advertised ceiling with the answer itself dropped. *Probability:* certain for any
repository carrying a config file with keys this version does not know — which is
what happens on the first day the key set changes. *Fix cost:* one function.
Cap `receipt_lines`' warning list at a small constant with an "and N more" line, and
emit config warnings **after** `ENVELOPE.banner` rather than before it — the same
edit closes the B16-H3 injection surface, because repository-authored key names stop
appearing in the region the design reserves for tool-authored text. Add the two tests
B16 already drafted (5,000 unknown keys must stay ≤ `max_tokens`; `wrap` with
`max_tokens` below the header size must not exceed `max_tokens`).

**3. Apply the existing containment check to `walk_repo`'s git branch (g).** *Blast
radius:* arbitrary readable file content outside the root, rendered to the agent
through `repo_map`, `find_symbol`, `find_referencing_symbols` and the on-disk index.
*Probability:* moderate — it needs a repository with a committed absolute symlink,
which survives `git clone` and is the one CRITICAL here reachable from content alone.
*Fix cost:* three lines. `fslimits._file_stays_inside` already exists and already
does exactly the right thing; call it from `treewalk.py:93` in place of the bare
`candidate.is_file()`, and report the skipped entries through the `skipped` channel
`refs.py` already carries.

**4. Give numeric bounds one owner (S2, covering i and j).** *Blast radius:* six
tools answer confidently and wrongly — "no references" for a symbol with 52 sites at
`limit=0`, the whole file for a 21-line slice at `context_lines=-50`, a negative
limit echoed back into a user-facing receipt. *Probability:* high; these are
model-generated argument lists and nothing on the wire rejects them. *Fix cost:*
moderate but mechanical. Put the guard in the service methods (the boundary both
adapters cross — `core/mermaid.py:253` already chose that side and is the model),
then annotate the wire with `Field(ge=…)` as schema documentation, since a schema
refusal is the only message a model can act on before the call. Pair it with S1's
narrow half: make `render_ref_groups` take the listing (`total`, `limit`) rather than
the truncated tuple, so `render.py:788` cannot recompute a total that asserts
completeness.

**5. Decide the `--root` model and encode the decision (a).** *Blast radius:* the
tool's only authorization boundary, plus three docstrings that state the opposite of
the code. *Probability:* certain to matter the first time an operator relies on
`--root`. *Fix cost:* small once decided — either intersect (`candidates` in
`_sole_selection` already computes the intersecting form) or keep the union behind an
explicit `--allow-client-roots` and rewrite the three docstrings. Remove the
`client_roots[0]` fallback at `server.py:116-117` either way: defaulting to a
directory that matches *no* configured root contradicts the same function's own
docstring three sentences earlier and is not defensible under either model.

**Ride-alongs — do these in whichever PR is already open, they are one line each:**
add `-c core.hooksPath=…` to `sandbox.run_git`'s fixed argv (d, plus 51× per validate
run); pass `newline=""` at `patch_service.py:343` (Independent Finding #1); catch
`OSError` rather than only `BlockingIOError` at `filelock.py:85` (Independent
Finding #2).

---

## Proportionality Assessment

**Proportionate, adopt as written.** S2's fix direction (one owner per bound at the
service, `Field` on the wire as defence in depth), S1's `SharedCallerListing`-shaped
listing type, B07-C1's `except`-tuple widening, B16's five adversarial tests, and
B22's "decide the model, then enforce it" are all the smallest change that removes
the failure. B07-C1's recommendation is notable for separating the cheap containment
from the expensive rewrite; that is the shape the rest should follow.

**Disproportionate — take the cheaper alternative the block reviewer already
offered.** B08-C1's primary recommendation is to hold an explicit `BEGIN` across a
`CachedSource`'s lifetime. That pins a WAL read snapshot for the duration of a `map`
or a repo-wide `refs` scan on a large repository, which blocks checkpointing and
grows the WAL — a new operational failure mode traded for a narrow race. The
reviewer's own fallback is two lines and has no such cost: **treat a zero-row result
as a miss and fall through to the extractor**, which is exactly the "could not
measure ≠ measured negative" rule S4 argues for everywhere else. Take the fallback.

**Disproportionate — declines to fix.** The panel's recommendation to replace
`sandbox._WORKTREE_LOCK`'s `threading.Lock` with `util/filelock`
(`panel/reliability.md:219`, B14) buys nothing here: the race being guarded is
`validate --jobs N` inside a single process, which a `threading.Lock` fully covers,
and a cross-process file lock would have to live somewhere — and every candidate
location is either the analysed repository (the thing B14-H1 objects to writing into)
or the cache directory, which does not scope to the repository being worktree'd.
Keep the `threading.Lock`; the half of that recommendation worth taking is running
`worktree prune` only on the fallback branch.

**Disproportionate as stated, proportionate if re-scoped.** S3 proposes correcting
all twenty-two false docstrings ("a day's work") plus four gates. Correcting prose
without a gate re-creates the class the moment the next change lands, and this
codebase's own rule is that a stated guarantee names the gate that enforces it. Cut
the prose pass to the load-bearing subset — the ones a maintainer or the next agent
will *generate against*: `cache.py:22-30`, `validate_service.py:29-33`,
`repo_context.py:10-14` / `server.py:8-13,96`, `extractor.py:481-486`,
`fslimits.py:1-14`, and `contained_path`'s misdirecting dead re-resolve
(`fslimits.py:48-57`, B01-M3), each corrected in the same commit as the code fix
above that made it false. Leave the remaining sixteen for the gates.

**Where the panel under-costed a fix.** Nowhere significant, with one exception: the
`extractor.py` split (B07-M6, `panel/architect.md`) is described as "the single
highest-value mechanical refactor in the repo's dependency graph". It may well be,
but it requires first relocating `LANGUAGE_CONFIGS`, `COMMENT_NODE_TYPES`,
`INDENT_BLOCK_NODE_TYPES` and `BODY_BLOCK_NODE_TYPES` — imported by `core/normalize.py`
and `core/skeleton.py` — which makes it a three-block change with golden-file
consequences. It is not a prerequisite for any of the Top 5 and should not be bundled
with the recursion fix.

**Cost context for all of the above.** The suite is 1,141 tests in 16.28 seconds and
green at `bf7b21b`. Every fix in the Top 5 is regression-protectable in the same
commit at negligible cycle time. The absence of any CI (`ls -d .github` → does not
exist, verified in synthesis) is what makes that cheapness theoretical rather than
enforced, and is the reason gates proposed in prose keep re-appearing as findings.
