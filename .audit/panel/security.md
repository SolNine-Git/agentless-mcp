# Security Panel Review — agentless-mcp

Reviewer: SECURITY_AUDITOR (independent panel validator)
Scope: full `.audit/findings/` corpus (B01–B24), evaluated against the MCP-server /
untrusted-analysed-repo / LLM-authored-patch threat model.
Method: derived from the 24 per-block reviews' own reproductions (each finding below
cites the block finding(s) it is built on); no new exploitation was attempted beyond
re-reading the quoted evidence. Severities and framing are this reviewer's own and
diverge from the block reviewers' where the security impact differs from the
correctness impact they scored.

---

finding_id: 1
persona: SECURITY_AUDITOR
severity: CRITICAL
scope: cross-module
files: [src/agentless_mcp/adapters/mcp/server.py:187, src/agentless_mcp/application/repo_context.py:11-14,96-98]
category: access-control
issue: The MCP server's stated confinement boundary — "a server started with --root /a serves /a and nothing under, beside or symlinked from it" — is not enforced. `server.py:187` unions client-advertised MCP workspace roots into the authorization allowlist before calling `resolve_repo`, and `_authorise` (`repo_context.py:96`) accepts any exact match against that unioned list. A server started with zero `--root` flags serves any directory the connected client advertises. This is the MCP tool-schema trust boundary named in the brief, and it is broken in the permissive direction: the client, not the operator, ends up deciding what is servable.
evidence: Reproduced in B16-H1 and independently in B22-H1/B22-M5 with a real `ToolHandlers` instance — a server configured with `--root /tmp/.../configured` still accepted and served `/tmp/.../never-configured-secret` when that path was advertised as a client root, and a server with zero configured roots served whatever the client advertised. `repo_context.py:11-14` and `server.py:8-13` both assert the opposite in prose (B16-H1, B22-M5): "checked, exactly, against the roots the server was started with." `tests/unit/test_mcp_server.py:118` pins the widening as intentional, so this is deliberate design that contradicts its own documentation, not a one-line regression.
source: OWASP API1:2023 Broken Object Level Authorization / A01:2021 Broken Access Control — the enforcement point trusts a client-supplied value as if it were server-side policy. Security Engineering (Ross Anderson), ch. on access control: the trusted computing base must not include a value the "untrusted" side of the boundary supplies.
recommendation: Decide the model and enforce it in code, not prose. If `--root` is meant to be confinement (the only reading consistent with `_sole_selection`'s existing "select among configured roots" logic and with the docs), intersect rather than union: a client-advertised root may only select among statically configured roots, never add to them; refuse to serve when zero static roots are configured and no advertised root matches, rather than falling through to "any directory." If additive client-authorization is genuinely the intended product design, it must be a loud, documented, opt-in flag (`--allow-client-roots`), not the silent default, and every "confinement" claim in `repo_context.py`, `server.py`, and `docs/agent-guide.md` must be rewritten to match.
confidence: HIGH

---

finding_id: 2
persona: SECURITY_AUDITOR
severity: HIGH
scope: module
files: [src/agentless_mcp/adapters/mcp/server.py:443-464]
category: injection
issue: `effective_client_roots()` parses the untrusted MCP `roots/list` response with no single parse-or-raise boundary step. An empty or malformed `file:` URI (bare `file://`) resolves to the server process's current working directory via `Path("").resolve()`, silently authorizing whatever directory the server happened to be launched from. A URI with a non-empty authority (`file://host/etc`) has that authority silently discarded and is treated as a local path. `file:///` resolves to `/`, which combined with finding #1 authorizes the entire filesystem. No result validation (existence, directory-ness) occurs at all.
evidence: Measured in B22-H2: `urlparse('file://')` → empty path → `Path("").resolve()` → the server's launch cwd; `urlparse('file://host/etc')` → authority `host` dropped, path resolves to `/etc`; `file:///` → `Path("/")`. None of these are logged above DEBUG/INFO. This is the block's single parse of foreign data into a value that grants authorization, and it fails the boundary-integrity rule on every count named in the brief: no typed refusal, malformed input coerced into a plausible value.
source: OWASP A03:2021 Injection (URI/path parsing without validation) and Boundary integrity — "Foreign data crosses into the system through one parse step that converts it to typed domain values or raises." RFC 8089 requires rejecting a non-empty, non-localhost authority rather than silently discarding it.
recommendation: Parse-or-drop with an explicit refusal: require `scheme == "file"`, `netloc in ("", "localhost")`, a non-empty absolute `path`, and confirm the resolved path exists and is a directory before it ever reaches the allowlist. Log every rejection at WARNING with the offending URI. This closes the parsing hole regardless of how finding #1 is resolved.
confidence: HIGH

---

finding_id: 3
persona: SECURITY_AUDITOR
severity: CRITICAL
scope: module
files: [src/agentless_mcp/core/sandbox.py:184-202]
category: injection
issue: `git worktree add` is executed against the analysed repository with no hook or config isolation, so a hostile repository's committed `.git/hooks/post-checkout` (or any other worktree-triggered hook) runs with the invoking user's privileges the moment a worktree is created — before any patch or test command is involved. This fires on both the `patch apply` (default worktree) path and every `validate` candidate. The same file's `diff()` function explicitly neutralises `color.diff`/`diff.external` twelve lines away "because a worktree reads the repository's own configuration," proving the hazard was known and the mitigation was applied to one call and not the other.
evidence: Reproduced in B14-M2: a `post-checkout` hook containing `echo PWNED > ../pwned.txt` executed successfully (`exit=0`) during `git worktree add --detach ../hookwt HEAD`, writing the file outside the worktree. This directly contradicts sandbox.py's own module docstring ("Every git call is a fixed argv with a timeout and no shell. Nothing derived from repository content or from patch text reaches it.") and the module's advertised read-only posture. The threat model explicitly names "the ANALYSED REPOSITORY is itself untrusted input" — this is that exact input reaching code execution.
source: Full Stack Python Security (Dennis Byrne), ch. 12.2 "Invoking external executables" — an external command that consults attacker-controlled configuration is equivalent to attacker-controlled code, whatever the argv looks like. OWASP A03:2021 Injection / CWE-78.
recommendation: Pass `-c core.hooksPath=/dev/null -c core.fsmonitor=` (and any other repository-config-driven hook surface) on every `git worktree add` invocation, matching the hardening already applied to `diff()`. Document the residual risk from any locally-installed (not repo-committed) hooks path. Treat this as the single highest-priority fix in the sandbox module — it is reachable from the default `patch apply` code path, not only from `validate`.
confidence: HIGH

---

finding_id: 4
persona: SECURITY_AUDITOR
severity: CRITICAL
scope: cross-module
files: [src/agentless_mcp/core/sandbox.py:205-234, src/agentless_mcp/application/validate_service.py:29-33, src/agentless_mcp/adapters/cli/main.py:1005-1019, src/agentless_mcp/adapters/cli/main.py:507-518]
category: injection
issue: `validate` executes `test_cmd`, and `test_cmd` falls back to a value read from the analysed repository's own `.agentless-mcp.json` (`ctx.config.test_cmd`) whenever the caller does not supply `--test-cmd` explicitly. The repository under judgment names its own judge, and that command runs with the user's/agent's full privileges and environment. The module docstring at `validate_service.py:29-33` states as an architectural guarantee that "nothing here reads a test command out of the repository under analysis... letting it nominate its own judge is the injection path this whole package is shaped to avoid" — a claim the shipped CLI directly violates. The CLI's own docstring separately claims the fallback runs "only when a human asked for a validation run... and can see which command it chose," which is also false: the CLI is documented elsewhere as "the front door any agent can reach over Bash," so an autonomous agent can trigger this without a human ever reading the printed command.
evidence: B14-M3, B21-M4, and B23-H5 each independently identify the same mechanism from three angles (sandbox execution, service docstring, CLI mitigation-claim). `main.py:1005`: `test_cmd = args.test_cmd if args.test_cmd is not None else ctx.config.test_cmd`; the only mitigation is a `note()` written to stderr immediately before the process is spawned — not a confirmation, not an opt-in flag, not a gate a reviewer could point to. `shlex.split(cmd)` with no shell (`sandbox.py:236`) means the argv is exactly what the repository's config named; there is no further sandboxing of what that argv can do once it runs.
source: Full Stack Python Security, ch. 12.2 "Invoking external executables" — "a command string from an external source" reaching `subprocess`/`Popen` is the canonical case this chapter warns against, independent of shell-injection specifically. OWASP A08:2021 Software and Data Integrity Failures (the "judge" is supplied by the thing being judged).
recommendation: Require an explicit, separately-named opt-in (`--allow-config-test-cmd`) before the config-file fallback is used at all, rather than treating it as the default with an advisory note. Rewrite both docstrings (`validate_service.py:29-33` and the CLI's) to describe what is actually enforced instead of a guarantee the code does not hold — per the audit's own Pass-1 rule, a false invariant in a docstring is scored at the severity of the bug it induces, and an agent generating code or reasoning against this module will read the false guarantee and rely on it.
confidence: HIGH

---

finding_id: 5
persona: SECURITY_AUDITOR
severity: HIGH
scope: module
files: [src/agentless_mcp/core/treewalk.py:79-104, src/agentless_mcp/core/refs.py:115-118, src/agentless_mcp/core/cache.py:640-645]
category: access-control
issue: `walk_repo`'s git-listed path serves any file `git ls-files` names, including a tracked symlink whose target resolves outside the repository root — the only filter applied is `candidate.is_file()`, which follows the link. This directly contradicts the containment guarantee the same repository enforces on every other read path (`fslimits.contained_path`, `read_slice`, `skeleton`), and it contradicts `repo_context.py`'s stated model that the server serves configured roots "and nothing... symlinked from them." The analysed repository — explicitly named as untrusted input in the threat model — fully controls what gets symlinked and committed.
evidence: Reproduced end-to-end in B04-C1: a repo with a committed symlink `leak.py -> /tmp/.../secret.txt` produced `walk_repo(root)` output including `('leak.py', 10)` — the size of the *outside* target — and `read_bounded(root / "leak.py")` returned the outside file's content, `TOPSECRET`. This content then flows into `repo_map`, `refs`, and the persistent tag-cache index (symbol names, imports, signatures extracted from the out-of-root file and served through subsequent tool calls). The equivalent single-path read is refused by `contained_path`; only the bulk-listing/index path is unguarded.
source: CWE-59 (Improper Link Resolution Before File Access, "Link Following") / OWASP A01:2021 Broken Access Control. Security Engineering ch. on access control: a boundary enforced at one door and not at another door onto the same resource is not a boundary.
recommendation: Apply the same containment test the non-git fallback path already uses (`fslimits._file_stays_inside` / `_is_within`) to every path returned by `_git_listed_paths` before it reaches `candidate.stat()` or `read_bounded`. Promote the helper to a shared, public function so both walk paths use one implementation, and add a regression test that commits an escaping symlink and asserts it is absent from `walk_repo`'s output.
confidence: HIGH

---

finding_id: 6
persona: SECURITY_AUDITOR
severity: HIGH
scope: cross-module
files: [src/agentless_mcp/application/envelope.py:84-86,127,136,145-148, src/agentless_mcp/core/projectconfig.py:199-205]
category: access-control
issue: `.agentless-mcp.json`'s unknown-key warnings are unbounded (one line per unknown key, no cap, unlike the `stoplist` field in the same file, which is capped) and are folded into the response header *before* the token-budget accounting runs (`budget = max_tokens - counter.count(header)`, with no floor). A hostile or merely oversized config file — well within the existing 64 KB size cap — therefore lets the analysed repository consume the entire output ceiling of *every* subsequent tool call against it and delete the answer body while the truncation note honestly (and misleadingly) blames the ceiling rather than the repository's own config file. This is a repository-content-triggered denial of service on the read surface, reachable through the single most commonly exercised trust boundary (config parsing) named in the brief.
evidence: Measured in B16-C1: 8,000 short unknown keys in a `.agentless-mcp.json` under the 64 KB cap produced 301,789 tokens of header against a `max_tokens=16000` request, with the body 100%-dropped and the receipt honestly reporting "100 of 100 lines dropped" — a confident-looking but content-free answer from every tool called against that repository from then on. The JSON path is worse: 287,887 tokens with `"shown": 0, "total": 10`.
source: OWASP A04:2021 Insecure Design (missing resource consumption limits) / CWE-400 Uncontrolled Resource Consumption. Same principle the codebase itself already applies to `stoplist` (`projectconfig.py`'s own `MAX_STOPLIST_ENTRIES`) and to file size and walk bounds (`util/fslimits.py`) — this is the one repository-controlled channel into every response that was left uncapped.
recommendation: Cap `ctx.config.warnings` at a small fixed count (the block review suggests 8) plus a "N further warnings suppressed" line, in `projectconfig.parse` itself — the same place `MAX_STOPLIST_ENTRIES` already lives. Independently, clamp the assembled header as a whole (`_fit(header, counter, max_tokens // 4)` or similar) so no single code path can ever emit more than `max_tokens` regardless of what a future header field adds. Defense in depth: bound both the producer and the consumer.
confidence: HIGH

---

finding_id: 7
persona: SECURITY_AUDITOR
severity: HIGH
scope: module
files: [src/agentless_mcp/application/envelope.py:84-86,127, src/agentless_mcp/core/projectconfig.py:203]
category: injection
issue: The envelope's stated defence against prompt injection is a banner that "marks everything below it as repository data... Rendered source is untrusted input... the banner is what keeps it one" — but repository-controlled text (config-key names, verbatim modulo `repr`) is rendered *above* that banner, in the region the design reserves for tool-authored text. Combined with finding #6's unbounded warning count, a hostile repository can place unbounded lines of its own content in the position an LLM agent is meant to trust as coming from the tool itself, not from the analysed repository — a textbook indirect prompt-injection surface (MITRE ATLAS AML.T0051.001).
evidence: B16-H3 confirms `receipt_lines` (which includes `projectconfig.py:203`'s `f"unknown key {key!r} in {CONFIG_FILENAME}: ignored ..."`) is placed before `ENVELOPE.banner` in `header = "\n".join([*receipt_lines(ctx), ENVELOPE.banner, ""])`. Mitigating factor, noted for calibration: `{key!r}` escapes newlines/quotes, so a single warning line cannot forge a whole receipt line or close the banner early — the injection is confined to a fixed template shape, not free text. But volume alone (finding #6) turns "confined to a template" into "thousands of attacker-chosen substrings appearing in the trusted region," which is why this is filed separately at HIGH rather than folded into #6's availability framing.
source: MITRE ATLAS AML.T0051.001, "LLM Prompt Injection: Indirect" — content from an external, untrusted source reaching model context without a reliable trust label. OWASP LLM01:2025 Prompt Injection.
recommendation: Move config warnings below the banner, alongside the truncation notes (which are already correctly positioned). If a receipt-region field is ever needed to carry repository-derived text again, wrap it in an explicit, distinct marker so it cannot be confused with tool-authored receipt lines even before the banner question is settled.
confidence: MEDIUM

---

finding_id: 8
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/projectconfig.py:145-147]
category: access-control
issue: `projectconfig.load` reads `.agentless-mcp.json` via `path.is_file()` / `read_text()`, both of which follow symlinks, and the read is never routed through `contained_path`. A repository can therefore commit `.agentless-mcp.json` as a symlink to a file outside the repository root; that file's key names, parse errors, and any settings it successfully sets (`test_cmd`, `stoplist`, budgets) are read and surfaced. This directly contradicts the module's own stated defence: "No key is path-typed: a repository cannot name a file for this tool to read, which removes the whole class of 'config points somewhere else' escapes before it starts" — a symlinked config file *is* a repository naming a file to read, closed at the key level and left open at the file level.
evidence: Reproduced in B04-M1: `repo2/.agentless-mcp.json -> /tmp/.../outside.json` was read successfully, `config.present` was `True`, and the outside document's key names appeared in the receipt's unknown-key warnings.
source: CWE-59 Improper Link Resolution Before File Access. Lower severity than finding #5 because the leak channel here is limited to key names, JSON parse-error text, and resulting settings values — not full file content, and `test_cmd` in particular is already covered by finding #4's stronger issue.
recommendation: Refuse the config when `path.is_symlink()` and the strict resolve is outside `repo_root`, or route the read through `fslimits.contained_path(repo_root, CONFIG_FILENAME)` and turn a `SecurityRefusal` into a warning (preserving the "never fails a read command" property the module otherwise holds).
confidence: HIGH

---

finding_id: 9
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: module
files: [src/agentless_mcp/adapters/mcp/server.py:116-117]
category: access-control
issue: `_sole_selection`'s fallback branch returns a single client-advertised root as the default repository whenever it identifies *zero* configured roots (`if not candidates and len(client_roots) == 1: return client_roots[0]`), which is the opposite of the documented rule ("identifies exactly one configured root," `docs/agent-guide.md:136-140`). An omitted `repo_root` argument can therefore silently resolve to a directory the operator never configured with `--root`, in preference to the one they did configure, whenever exactly one client root happens to be advertised and it does not match.
evidence: B22-M4 measured it directly: static roots `[/a]`, client advertises `[/b]` (unrelated); the selection returns `/b`, not a refusal. The receipt does name the selected root, which keeps this from data exfiltration on its own, but it silently substitutes the operator's configured target with an unrelated, client-controlled one for calls that omit `repo_root` — the common case for an agent that has not been told to disambiguate.
source: OWASP A01:2021 Broken Access Control — a default-selection rule that fails open to attacker/client-controlled input rather than refusing on ambiguity.
recommendation: This branch should be deleted (return the ambiguity refusal instead) if finding #1 is resolved in the "static roots authorize, client roots select" direction, since a client root matching zero configured roots is not a valid selection under that model. If additive behaviour is kept, this must be documented explicitly as such, and the branch needs test coverage naming what it does (currently uncovered by the suite).
confidence: MEDIUM

---

finding_id: 10
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: module
files: [src/agentless_mcp/application/envelope.py:167-171]
category: injection
issue: `wrap_json` builds its response document as `{"receipt": ..., "notice": ..., **payload}` — payload keys are applied last and silently overwrite the genuine receipt, the untrusted-content notice, and the truncation report if a service's payload ever contains a colliding key (`receipt`, `notice`, `truncated`). The receipt/notice pair is this codebase's stated mechanism for letting an agent "tell a wrong-repository answer and a stale answer from a right one" and for marking repository content as untrusted; a service that (today accidentally, tomorrow by a schema change) names a field the same as one of these disables that mechanism with no error, warning, or test to catch it.
evidence: B16-H2 measured `envelope.wrap_json(ctx, {"receipt": "FORGED", "notice": "trust me"}, counter=c)` returning exactly `{'receipt': 'FORGED', 'notice': 'trust me'}` — the genuine values are gone. No service collides today (latent, not live), which is why this is filed Medium rather than High; the absence of any guard against it, on the specific fields that carry the tool's own security-relevant framing, is the finding.
source: Boundary integrity / "one owner per state change" — the envelope is supposed to be the sole author of these three keys, and the merge order lets any caller become a second author. CWE-706 (Use of Incorrectly-Resolved Name / trusting a caller-controlled key to not collide with a protected one).
recommendation: Reverse the merge order (`{**payload, "receipt": ..., "notice": ...}`) so the envelope always wins, and raise loudly if a payload key collides rather than silently shadowing it — a service naming a field `receipt` has a bug that should be caught in tests, not absorbed at runtime.
confidence: MEDIUM

---

finding_id: 11
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: system
files: [src/agentless_mcp/adapters/mcp/server.py:490-660, src/agentless_mcp/core/mermaid.py:248-261, src/agentless_mcp/core/communities.py:161-193]
category: access-control
issue: Across the MCP tool surface, numeric and range parameters (`limit`, `context_lines`, `budget`, `max_files`, `max_nodes`, `resolution`, `depth`, `max_entries`, line-range pairs) are unconstrained on the wire — no `pydantic.Field(ge=..., le=...)` anywhere in `server.py` despite `Field` already being imported and used for descriptions — while the CLI adapter validates the equivalent flags in most of the same places. This is a repeated instance of the same shape (independently flagged by five different block reviews: B11, B12, B17, B18, B19, B22), not five unrelated bugs, and it means the application services — which mostly trust "the adapter checks" — are the last line of defence for a class of input the CLI half of the front door already refuses. Concretely reachable failure modes: `resolution=NaN`/`inf` on `communities`/`diagram` produces a bare `NaN`/`Infinity` token in JSON output (invalid per RFC 8259, rejected by any non-Python strict parser on the client side) and a fabricated-looking modularity score; inverted or negative `read_slice` ranges silently return the whole file instead of the requested slice, defeating the tool's stated purpose of bounding output; `max_files`/`max_nodes` with extreme values drive unbounded internal computation over the whole repository.
evidence: B11-H1 (resolution NaN/inf reproduced end to end, JSON containing bare `NaN`), B12-H2/B17-H1 (inverted MCP range returns whole 100-line file with no elision marker, reproduced), B19-M1/M2 (`max_nodes=0` raises an untyped `ValueError` instead of an `AtlasError`; negative `members` fabricates an "omitted" count), B22-M1/M2 (three-element or malformed `lines` pairs silently dropped, collapsing to "no intervals" and returning the whole file), B18-M2 (`limit=0`/negative on `find_referencing_symbols` produces a confident false-negative "no references" answer).
source: OWASP A04:2021 Insecure Design / CWE-20 Improper Input Validation, and CWE-1284 (Improper Validation of Specified Quantity in Input) for the resolution/max_nodes cases specifically. Least-privilege framing: an MCP client is explicitly the untrusted party in this threat model, and the surface that receives its input directly is the one with the least validation in the codebase.
recommendation: Add `Field(ge=1, le=<sane_max>)` (or the appropriate bound) to every numeric MCP tool parameter so the JSON-RPC schema itself rejects out-of-range calls before they reach a service — this is also the only enforcement point a client can introspect and self-correct against. Where a service already validates (mermaid's `max_nodes`), keep that check as the domain-level backstop; where none exists (symbol_service slicing, communities resolution), add it there too, since the CLI and any future adapter both need it and it should not be re-derived per adapter.
confidence: HIGH

---

finding_id: 12
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: module
files: [src/agentless_mcp/adapters/mcp/server.py:451,483,657]
category: access-control
issue: `context.list_roots()` — a JSON-RPC round trip back to the connected MCP client, made on the critical path of every one of the eleven published tools via `context_for` — is awaited with no timeout (`asyncio.timeout`/`wait_for`) anywhere in the module, violating the stated global invariant that every out-of-process/network call gets a timeout. The exception handling is also narrower than the failure surface: only `McpError` is caught; a client answering `roots/list` with a payload that fails pydantic validation, or a transport that dies mid-request, raises an uncaught exception instead.
evidence: B22-H3: the code at `server.py:451-464` has no `asyncio.timeout` around the `await context.list_roots()` call, and the surrounding docstring only addresses the `McpError` "no capability" case, not a hang or a malformed response.
source: Engineering Invariants — "External calls fail, hang, or duplicate... every network or out-of-process call gets a timeout." CWE-400 (resource exhaustion via unbounded wait) applied to availability rather than confidentiality/integrity: a client that never answers `roots/list` (a real class of early-MCP-client bug) hangs every tool call indefinitely with no visible symptom beyond "the tool never returns."
recommendation: Wrap the await in a short `asyncio.timeout` (a client capability query should answer in well under a second) and fall back to static roots on expiry, the same way the `McpError` path already does. Broaden the except clause to cover payload-validation and transport failures with the same fallback.
confidence: MEDIUM

---

finding_id: 13
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/sandbox.py:87-89,244-255,364-380, src/agentless_mcp/adapters/cli/main.py:1398-1406]
category: access-control
issue: Captured stdout/stderr from a candidate's test run is written to an unbounded `tempfile.TemporaryFile` for the entire `timeout` window (default 300s, with no enforced ceiling on `--timeout` beyond `> 0`); the `DEFAULT_MAX_CAPTURE` bound is applied only when the tail is read back afterward, not while the child is writing. A test suite (from an analysed repository, which is untrusted input) that prints or dumps large output at disk/tmpfs speed for the full timeout window can exhaust `/tmp` — RAM on the many systems where `/tmp` is tmpfs — and `--jobs N` multiplies this by `2N` concurrent streams.
evidence: B14-H2: "the comment says 'cap on captured output'; the code caps only the tail `_tail` reads back," measured against the module's own stated thesis that the timeout bound is the hard guarantee this machinery provides.
source: CWE-400 Uncontrolled Resource Consumption / OWASP A04:2021 Insecure Design. This is availability impact triggered by hostile-repository test-suite behaviour, consistent with the "what can a hostile analysed repository make the tool do" framing in the brief.
recommendation: Bound the writer, not just the reader — set `RLIMIT_FSIZE` on the child via `preexec_fn`/`resource`, or have the wait loop stop/truncate the process once its temp file passes a small multiple of `max_capture`. Cap `--jobs` (e.g. at `os.cpu_count()`) so worst-case disk consumption is bounded, since each job holds a full concurrent capture stream on top of a full worktree checkout.
confidence: MEDIUM

---

finding_id: 14
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: cross-module
files: [src/agentless_mcp/core/grammars.py:258-268, src/agentless_mcp/bootstrap.py:67-77]
category: dependencies
issue: Both of this package's network-fetching dependencies are called with no timeout and, on failure, surface exceptions outside this package's typed error hierarchy. `grammars.py`'s only network call (`pack.prefetch`) has no timeout knob available from the underlying `tree-sitter-language-pack` API and none is imposed at this call site, so a stalled connection (captive portal, silently-swallowing proxy) hangs `agentless-mcp warmup` indefinitely. `TiktokenCounter.__init__` calls `tiktoken.get_encoding(...)`, which performs an unguarded `requests.get` on a cold cache; only the *import* of `tiktoken` is wrapped in the package's `AtlasError` handling, so a network failure or hang during the encoding fetch raises a raw `requests` exception straight out of the composition root with no timeout and no actionable message.
evidence: B06-M3 ("the download has no wall-clock bound anywhere in the stack" — verified against `inspect.signature(pack.prefetch)` and `dataclasses.fields(PackConfig)`, neither of which exposes a timeout knob) and B24-M1 (`.venv/.../tiktoken/load.py:17`: `resp = requests.get(blobpath); resp.raise_for_status()`, unguarded, and `cli_main`'s `except AtlasError` at `bootstrap.py:112` does not catch the resulting `ConnectionError`/hang).
source: Engineering Invariants — "External calls fail, hang, or duplicate... every network or out-of-process call gets a timeout." Dependency-trust framing: both fetches pull code/data (a parser grammar binary, a BPE encoding table) over the network with no integrity check visible in this codebase's control beyond whatever the upstream libraries do internally, and no bound on how long a failure takes to surface.
recommendation: Run `prefetch` under an explicit deadline (worker thread/subprocess with a wall-clock bound) at the `grammars.py` call site, converting expiry into the same degraded-row shape a `pack.Error` already produces. Wrap `tiktoken.get_encoding` in the same `try` that already guards the import, converting any exception (not just `ImportError`) into an `AtlasError` naming the encoding and pointing at `TIKTOKEN_CACHE_DIR` for offline pre-seeding.
confidence: MEDIUM

---

finding_id: 15
persona: SECURITY_AUDITOR
severity: MEDIUM
scope: module
files: [src/agentless_mcp/core/treewalk.py:55-87]
category: data-exposure
issue: `is_git_repo` decides the git-vs-fallback walk branch by testing for a `.git` entry at exactly the given root (`(root / ".git").exists()`), rather than asking whether the root is inside a git work tree. When the configured/requested root is a subdirectory of a monorepo (an explicitly supported configuration per `core/sandbox.py:160-165`), this proxy check fails, the code falls back to a walk that consults no gitignore state, and — measured — a vendored inner repository under that subdirectory is walked whole, including `vendored/.git/HEAD`, `vendored/.git/config` (a plausible location for an embedded remote URL with a credential), and every `vendored/.git/hooks/*.sample`. This content is then indexed and made available to the agent through `repo_map`/`refs`/symbol extraction as if it were ordinary source.
evidence: B04-H1: for `walk_repo(outer/"pkg")` with a vendored inner repo under `pkg/`, the walk returned 20 paths including the three `.git` internals named above; a top-level `.gitignore` excluding `node_modules/` was also silently ignored for the same reason.
source: CWE-668 Exposure of Resource to Wrong Sphere — data (VCS internals, potential credential-bearing config) that would normally be excluded by gitignore/convention becomes visible because the guard keyed on a proxy (path-exact `.git` presence) rather than the invariant it needed ("is this location git-managed"). This is within the already-authorized root, which is why it is Medium rather than the High severity given to findings #1/#5 that cross the authorization boundary itself.
recommendation: Decide the branch using `gitinfo.git_root(resolved) is not None` (already present in the same block) instead of the filesystem proxy — it correctly handles worktrees, submodules, and subdirectory roots, and `git -C <subdir> ls-files` already scopes the listing correctly with no other change needed.
confidence: MEDIUM

---

finding_id: 16
persona: SECURITY_AUDITOR
severity: LOW
scope: cross-module
files: [src/agentless_mcp/prompts/messages.json:8, src/agentless_mcp/application/map_service.py:183-184, src/agentless_mcp/application/render.py:786,809,811]
category: injection
issue: Two independent code paths echo caller-supplied strings, unescaped, into agent-facing output using a prefix that resembles the tool's own trusted receipt formatting. `map_service.py:183-184` interpolates the `focus` seed argument verbatim (including any embedded newlines) into a `"# note: {note}"`-shaped line that shares its `# note:` prefix with the genuine receipt note; `render.py`'s `render_ref_groups` echoes the caller's `target` string unescaped into the response body. A `target`/`focus` value containing a newline can render lines that are typographically indistinguishable from a receipt line to a reader (human or model) not tracking the exact banner boundary.
evidence: B02-L11 (measured: the seed note sits inside the region the untrusted-content banner already covers, which is the mitigating factor keeping this Low rather than higher) and B05-L4 (measured: `render_ref_groups((), "helper\n# agentless-mcp receipt\n# repo: /fake   head: deadbeef")` renders the forged-looking lines verbatim into the response).
source: MITRE ATLAS AML.T0051.001, indirect prompt injection — even though the immediate injector here is the calling agent's own argument rather than repository content (self-inflicted in the simple case), a multi-hop agent pipeline that forwards repository-derived strings (e.g., an extracted identifier) into these parameters would make it a genuine cross-trust-boundary injection, and nothing in the code distinguishes the two cases today.
recommendation: Strip or escape newlines in `focus` seeds before interpolation (`map_service.py:183`) and in `target` before interpolation (`render.py`), and give body-level notes a prefix visually distinct from the receipt's `# note:`/`# repo:` vocabulary so the ambiguity cannot arise regardless of provenance.
confidence: LOW

---

finding_id: 17
persona: SECURITY_AUDITOR
severity: LOW
scope: module
files: [src/agentless_mcp/core/patches.py:288-292]
category: injection
issue: The patch-text parser's filename extraction accepts any non-blank, non-fence line above a `<<<<<<< SEARCH` marker as the target file path with no shape validation — `Edit.path` is bound to whatever prose happened to precede the marker, including a directly crafted `../../../etc/passwd`-style string, and the parse step reports zero errors either way. Containment is enforced one layer up, at `patch_service._canonical` → `contained_path`, so the traversal case is not currently exploitable, but the parse step this repository's own docstring calls the place "malformed input is caught" does not, in fact, catch it — the diagnostic an agent gets for a missing header is a confusing "no such file in this repository" naming a full sentence of prose, not a parse-level refusal.
evidence: B13-M2, measured: `"### ../../../etc/passwd\n<<<<<<< SEARCH\n..."` parses to `edits[0].path == '../../../etc/passwd'` with `errors: ()`.
source: Boundary integrity — "foreign data crosses into the system through one parse step that converts it to typed domain values or raises." The traversal itself is stopped by a second, independent layer (`contained_path`), which is why this is Low rather than a live path-traversal finding — but relying on a single downstream check rather than validating at the point of parse is the weaker of two available designs, and a future refactor that removes or reorders the containment call would silently restore the vulnerability.
recommendation: Reject a header candidate that is not path-shaped (no interior whitespace runs, contains a `/` or a known extension) at parse time, reporting "block names no file" instead of accepting arbitrary prose as a path. This makes the parser fail closed independently of the downstream containment check, consistent with defense in depth.
confidence: MEDIUM

---

finding_id: 18
persona: SECURITY_AUDITOR
severity: LOW
scope: module
files: [src/agentless_mcp/util/fslimits.py:41-46]
category: access-control
issue: `contained_path` — the single documented boundary parse step for every path arriving from outside the process (MCP tool arguments, CLI paths) — raises an untyped `ValueError` rather than the module's own `SecurityRefusal` when the candidate path contains a NUL byte (`Path("a\0b").resolve()` raises `lstat: embedded null character in path`). Both adapters key their top-level error handling on the typed `AtlasError` hierarchy (`SecurityRefusal` is a member of it); a raw `ValueError` escapes that handling entirely.
evidence: B01-M1, measured: `contained_path(root, "a\0b")` raises a bare `ValueError`. JSON strings (the MCP transport's wire format) can carry embedded NUL characters, so a client sending a path argument containing one triggers this on the primary untrusted-input surface.
source: Boundary integrity — "a renamed field must fail at the boundary, not read downstream" generalized to any foreign value; the module's own contract states the *only* error type this function is documented to produce is `SecurityRefusal`. CWE-158 (Improper Neutralization of Null Byte or NUL Character) as the general shape.
recommendation: Wrap the resolve in `try/except (ValueError, OSError)` and re-raise as `SecurityRefusal` naming the rejected form generically. Impact is limited to an unhandled-exception response (a crashed tool call / traceback) rather than data exposure or code execution, which is why this is Low — but it is the one path this codebase explicitly promises never emits an untyped error.
confidence: HIGH

---

## Summary for synthesis

The two load-bearing security claims of this codebase — "the analysed repository's content never becomes code, and never leaves the authorized root" — each fail at least once, and both failures are in code paths that ship today, not hypothetical extensions:

1. **Authorization boundary (findings 1, 2, 9):** the MCP server's `--root` confinement is not enforced against client-advertised roots; the code and its own docstrings disagree about this within the same file.
2. **Code execution from untrusted repository content (findings 3, 4):** `git worktree add` runs the analysed repo's git hooks unconditionally on the default `patch apply` path, and `validate`'s test-command fallback lets the analysed repository nominate the command that judges it, contradicting an explicit architectural guarantee stated in the module's own docstring.
3. **Containment escape via symlinks (findings 5, 8):** the git-listed walk path and the project-config loader each independently fail to apply the containment check the rest of the codebase enforces.
4. **Repository-content denial of service (findings 6, 7):** an unbounded, repository-controlled warning channel can consume the entire response budget of every tool call and does so in the region the design reserves for trusted server text.
5. **A repeated, systemic pattern (finding 11):** the MCP adapter validates materially less than the CLI adapter across at least six independently-reviewed blocks, on the one input surface this threat model calls untrusted by name.

None of the CRITICAL/HIGH findings require chaining — each is independently reachable and each was reproduced (not merely inferred) by the block review it is sourced from.
