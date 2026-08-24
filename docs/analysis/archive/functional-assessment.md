> Archived 2026-08-24: superseded by
> [`docs/analysis/benchmark-methodology.md`](../benchmark-methodology.md) for the
> navigation evidence and by `CHANGELOG.md` for findings M1 to M3; the repository
> paths cited below are the ones that stood on the assessment date.

# Functional assessment

- Assessment date: 2026-08-20
- Assessed revision: `02c15e20` (`fix/audit-remediation`)
- Environment: Linux, Python 3.13.11, `uv` lockfile environment
- Initial working tree: clean

## Overall verdict

**Mostly functioning with bounded defects and evidence gaps. Confidence: high.**

The main read workflows, stable identifiers, graph views, cache invalidation,
path confinement, CLI-only patch validation, and the attached MCP integration
worked in direct testing. The implementation is unusually explicit about
evidence quality and output bounds, and the automated suite is broad. No
critical or high-severity security failure was confirmed.

The verdict is not "functioning as intended" for four reasons. A configured
test timeout starts termination at the requested deadline but is not a hard
wall-clock bound. Unavailable grammars can collapse symbol lookup into an
unqualified "no matching symbols" false negative. The packaged stdio console
entry point has no subprocess integration test and did not initialize in the
local direct-client probe, although the same failure reproduced with a minimal
FastMCP server and therefore is not attributable to this repository. Finally,
the recorded navigation and supply-chain evidence is not independently
reproducible from immutable artifacts named by this repository.

The correction from the original conversational assessment is important:
the `agentless-mcp` MCP server **is attached to the reviewing client**. Its
eleven tools were enumerated and invoked successfully. The attached connection
and a separately spawned `agentless-mcp-server` stdio process are distinct
integration paths.

## 1. Intended objectives

The project provides deterministic, model-free repository navigation and
patch-validation primitives for coding agents. Tree-sitter extracts symbols,
imports, references, and source structure. Application services turn those
facts into budgeted maps, bounded views, stable IDs, evidence-tiered fan-in and
fan-out, and graph operations. Separate CLI and MCP adapters expose the same
read services; patch parsing, linting, worktree-isolated validation, and voting
remain CLI-only.

The problem is context selection and validation, not autonomous reasoning. The
calling agent decides what a task means, which evidence matters, and what patch
to propose. The package aims to make the evidence bounded, attributable to a
repository generation, and reproducible without inserting another language
model into the retrieval path.

Explicitly outside scope are full language-server-quality binding, semantic
type analysis, model-authored summaries, an execution sandbox, secret
filtering for directed reads, MCP-side patch application, and proof that better
localization necessarily produces correct patches. Windows is documented as
best effort rather than a tested support target.

## 2. Capability assessment

| Capability or claim | Implementation evidence | Test/runtime enforcement | External research | Firsthand observation | Verdict |
|---|---|---|---|---|---|
| Budgeted repository maps with optional focus | `src/agentless_mcp/application/map_service.py:111`; personalized PageRank in `src/agentless_mcp/core/graph.py:204`; bounded packing in the map service | Characterization goldens and `tests/unit/test_graph.py`; schema caps `max_files` | [Aider's repository-map design](https://aider.chat/docs/repomap.html) is direct precedent; [PageRank](https://ilpubs.stanford.edu/422/) supports link-based ranking, not code-localization effectiveness by itself | Unfocused map ranked general infrastructure; `focus=["build_server"]` moved `server.py` to rank 1. Two repeated focused calls were byte-identical | Supported, subject to the unavailable-grammar finding |
| Tree, skeleton, and bounded slices | `src/agentless_mcp/application/view_service.py:141`; `src/agentless_mcp/core/treewalk.py`, `src/agentless_mcp/core/skeleton.py`, and `src/agentless_mcp/core/slices.py` | Characterization skeleton tests; file, range, depth, entry, and output caps | [Tree-sitter](https://tree-sitter.github.io/tree-sitter/) supports concrete syntax trees and robust incremental parsing; its [`ERROR` and `MISSING` nodes](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html#the-error-node) explain why syntax-tolerant structural views are plausible | Tree, overview, and line slice returned current source with receipts. `depth=999` and a slice without ranges were rejected | Supported |
| Symbol lookup, expansion, and stable-ID round trip | `src/agentless_mcp/core/symbols.py:269-315`; `src/agentless_mcp/application/symbol_service.py:201` | `tests/unit/test_stable_ids.py`; `tests/characterization/test_two_call_contract.py` | No external paper is needed to validate the ID encoding; usefulness is an interface-design claim | `find_symbol` returned `py:src/agentless_mcp/adapters/mcp/server.py::build_server`; `expand_symbols` and `resolve_locations` accepted it unchanged and returned the correct body/span | Supported |
| Evidence-tiered reference lookup and symbol explanation | `src/agentless_mcp/core/resolve.py`; `src/agentless_mcp/application/symbol_service.py:268`; `src/agentless_mcp/application/graph_service.py:134` | `tests/unit/test_ref_tiers.py`, `tests/unit/test_refs.py`, `tests/unit/test_shared_callers.py`, and graph-service tests | Static names and imports are useful retrieval evidence, but Tree-sitter is not a type resolver. No cited study validates this project's exact four tiers or multipliers | `explain_symbol` separated same-file, resolved-via-import, and unique edges. Fan-in found `serve` as a same-file caller and test callers as import-resolved | Architecturally sound; comparative effectiveness unproven |
| Paths, cycles, communities, and diagrams | Fewest-hop and Tarjan SCC logic in `src/agentless_mcp/core/resolve.py:444,518`; communities in `src/agentless_mcp/core/communities.py:161`; Mermaid bound in `src/agentless_mcp/core/mermaid.py:164` | `tests/unit/test_resolve.py`, `tests/unit/test_communities.py`, `tests/unit/test_mermaid.py`, and graph goldens | [Louvain/modularity](https://arxiv.org/abs/0803.0476) supports graph partitioning as a heuristic; it does not prove that a file partition is a software architecture | Path found one same-file hop `serve -> build_server`; cycles reported none; 117 files formed 27 communities at modularity 0.317; an eight-node diagram named 82 elided modules | Supported as bounded structural hints, not ground-truth architecture |
| Optional hash-invalidated SQLite index | Per-file SHA-256 gates and generation receipts in `src/agentless_mcp/core/cache.py:1-35,322-420`; cache path derived outside the repository | `tests/unit/test_cache.py:202,362-400,717-724` covers reuse, dirty files, and generation mismatch | Content hashes are an appropriate invalidation key; no external effectiveness claim is required | A cached query against a dirty fixture found the newly added `dirty_only` symbol when the warmed grammar directory was supplied. `capabilities` named the cache generation and dirty count | Supported when the needed grammar is available |
| CLI/MCP shared read behavior | Composition roots in `src/agentless_mcp/bootstrap.py:107-149`; MCP registration in `src/agentless_mcp/adapters/mcp/server.py:703-931`; CLI service container in `src/agentless_mcp/adapters/cli/main.py` | In-memory FastMCP round trips in `tests/unit/test_mcp_server.py`; import-layer contracts in `pyproject.toml` | [FastMCP tools](https://gofastmcp.com/servers/tools) documents signature-based schemas, validation, annotations, and structured results | Attached MCP and CLI `find-symbol build_server` had identical content apart from one trailing transport newline. An in-memory map probe was byte-identical | Supported for application behavior; subprocess stdio coverage remains incomplete |
| MCP read-only surface | The default `--surface v2` decorates five: `orient`, `symbols`, `read` in `_register_v2` plus `find_referencing_symbols` and `capabilities` in `_register_shared`; `--surface v1` decorates the eleven this row was measured against; annotation helper in `src/agentless_mcp/adapters/mcp/annotations.py:29` | Exact-set and annotation assertions in `tests/unit/test_mcp_server.py:317-351,924-928` | The [MCP tools specification](https://modelcontextprotocol.io/specification/2025-06-18/server/tools) defines schemas and annotation hints; annotations are descriptive hints, so the absence of write handlers is the stronger gate | Live inventory contained exactly eleven read tools and all eleven were called, measured against the v1 surface that was the only one at the time; the v2 default published since 0.4.0 exposes the same capability set through five. No patch, validate, vote, index, fetch, or command tool was present; an in-memory listing confirmed all read-only annotation fields | Supported |
| Root authorization and path confinement | Root allowlist in `src/agentless_mcp/application/repo_context.py:70-116`; resolved containment in `src/agentless_mcp/util/fslimits.py:29-60` | Root, symlink, traversal, and empty-allowlist tests in `tests/unit/test_repo_context.py`, `tests/unit/test_fslimits.py`, and `tests/unit/test_mcp_server.py` | MCP roots are workspace context, not a security boundary; the project's independent server allowlist is therefore the correct design ([MCP roots](https://modelcontextprotocol.io/specification/draft/client/roots)) | Attached MCP refused `/tmp`, named the authorized root, and rejected `../../../../etc/passwd` after resolution | Supported |
| CLI-only patch parse, lint, isolated validation, and voting | Patch parser in `src/agentless_mcp/core/patches.py:227`; patch service in `src/agentless_mcp/application/patch_service.py:273`; validation in `src/agentless_mcp/application/validate_service.py:448`; process runner in `src/agentless_mcp/core/sandbox.py:259` | Patch/lint goldens; `tests/unit/test_patch_service.py`, `tests/unit/test_validate_service.py`, `tests/unit/test_vote.py`, and worktree/process tests | The [Agentless paper](https://arxiv.org/abs/2407.01489) and [canonical implementation](https://github.com/OpenAutoCoder/Agentless) support localization, repair, filtering, and validation as a simple pipeline | A SEARCH/REPLACE patch parsed, passed syntax-delta checking, and validated in an isolated worktree. Baseline and reproduction behaved correctly. A repository-provided test command was refused without authorization | Supported, except the wall-clock timeout claim |
| Grammar pinning and air-gap behavior | Exact pack pin in `pyproject.toml:20-31`; no-download gate and explicit warmup in `src/agentless_mcp/core/grammars.py:161-280` | `tests/unit/test_grammars.py` and capability reporting | Tree-sitter supports the parsing model; the native bundle trust analysis is project-specific | Attached capabilities reported pack 1.14.3 and warmed Python plus other grammars. A deliberately cold, no-download fixture did not fetch | Implementation supported; audit provenance needs an immutable upstream reference |
| Deterministic bounded output | Sorted traversal/ranking, fixed tie breaks, service budgets, schema caps, and envelope ceiling in `src/agentless_mcp/application/envelope.py` | Goldens, ordering tests, PageRank/community determinism tests, and cap validation | Deterministic algorithms support repeatability but do not establish better patches | Repeated focused MCP map was byte-identical. Diagram and list bounds produced explicit elision/refusal markers | Supported for tested operations |

## 3. Findings

### M1. `--timeout` is not the documented hard wall-clock bound

**References:** `src/agentless_mcp/docs/agent-guide.md:773-776`,
`src/agentless_mcp/core/sandbox.py:97-100,259-327,407-439`, and
`tests/unit/test_sandbox.py:359-373`.

**Expected behavior:** The guide and runner docstring call `--timeout` a hard
bound. A caller supplying one second should be able to reason that the command
will consume approximately that bound plus negligible cleanup.

**Observed behavior:** A synthetic process ignored SIGTERM and slept. With
`--timeout 1`, validation correctly returned `timeout`, marked the baseline
`UNVERIFIED`, and evaluated no candidate, but its run record reported a duration
of 6.002 seconds. The implementation waits the configured second, then allows
`TERM_GRACE_SECONDS = 5.0` before SIGKILL. It can wait another five seconds if
the leader remains after SIGKILL.

**Why it matters:** Classification is safe, but capacity planning and
`--run-timeout` reasoning are not described accurately. Small per-command
timeouts can overrun by several multiples, and the active command can carry a
whole-run deadline beyond its advertised wall-clock limit.

**Evidence:** Direct command:

```bash
uv run agentless-mcp validate \
  --candidates /tmp/agentless-mcp-assessment.Ecj3WN/repo/candidates \
  --repo /tmp/agentless-mcp-assessment.Ecj3WN/repo \
  --test-cmd "python ignore_term.py" --timeout 1 --jobs 1
```

The process-group cleanup itself worked. The defect is the hard-bound claim,
not the timeout verdict or POSIX descendant cleanup.

### M2. An unavailable grammar can become a confident symbol-lookup false negative

**References:** `src/agentless_mcp/docs/agent-guide.md:877-892`,
`src/agentless_mcp/core/refs.py:96-130,247-258`,
`src/agentless_mcp/application/symbol_service.py:208-231`,
`src/agentless_mcp/application/map_service.py:82-98,168-185`, and
`src/agentless_mcp/adapters/mcp/server.py:369-382,446-449`.

**Expected behavior:** The guide says an unwarmed grammar degrades only that
language "with a message naming" the warmup command. A lookup should not make
"could not parse any candidate file" indistinguishable from "the repository
contains no such symbol."

**Observed behavior:** `scan_repo` correctly records `SkippedFile` entries
when `LanguageUnavailable` is raised. `find_symbol` discards `scan.skipped` and
returns only cards; the text renderer consequently says `no matching symbols`.
`MapResult.as_dict()` contains skipped reasons, but the text path used by the
CLI and MCP omits them. A cold, no-download Python fixture produced:

```bash
UV_CACHE_DIR=/tmp/agentless-assess-uv-cache \
TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=/tmp/agentless-mcp-assessment.Ecj3WN/cold-grammars \
AGENTLESS_MCP_NO_DOWNLOAD=1 \
uv run agentless-mcp find-symbol dirty_only \
  --repo /tmp/agentless-mcp-assessment.Ecj3WN/repo --no-cache
```

The result was `no matching symbols`, with no grammar warning. The same source
returned `py:core.py::dirty_only` once the warmed grammar cache was supplied.

**Why it matters:** This is an error-surfacing failure at the core navigation
boundary. An agent can treat a degraded scan as affirmative absence and make a
wrong localization or duplication decision. Render skipped-file reasons, or
refuse/qualify an answer when relevant files could not be parsed.

### M3. The packaged stdio entry point lacks an end-to-end transport gate

**References:** the live claim in `README.md:46-54`, entry point in
`pyproject.toml:42-43`, `src/agentless_mcp/bootstrap.py:130-149`, and the test description and
in-memory client usage in `tests/unit/test_mcp_server.py:1-3,306-310`.

**Expected behavior:** Because `agentless-mcp-server` is a published console
command and stdio is the documented transport, CI should start that command,
complete MCP initialization, enumerate tools, and execute one call over real
stdio.

**Observed behavior:** Existing MCP tests use `Client(server)`, FastMCP's
in-memory transport. No test spawns the installed console script. Locally,
both the official MCP Python client and FastMCP's stdio client started
`agentless-mcp-server` but did not complete `initialize` within 12-15 seconds.
The same hang reproduced against a minimal four-line FastMCP server under the
locked FastMCP 3.4.7 / MCP 1.29.0 stack.

**Why it matters:** The missing gate is confirmed; a project-specific runtime
defect is not. The minimal reproduction points to the dependency stack,
environment, or probe interaction rather than `agentless-mcp` handlers. The
attached MCP server worked fully, so this observation must not be generalized
to "MCP is broken." Add a subprocess transport test and resolve or document the
upstream compatibility before relying on the console stdio claim.

### L1. The 60-task navigation results are not reproducible from this repository

**References:** `docs/evidence.md:3-30,42-76,89-97`.

**Expected behavior:** Recorded effectiveness evidence should name an
immutable dataset revision and include or link the task-level inputs, proof
logs, and recomputation script needed to verify denominators and statistics.

**Observed behavior:** The document names a companion `swe-explore-bench`
dataset and relative `results_pilot/.../top10.jsonl` files, but this repository
contains no such files, no immutable URL or commit, and no bootstrap script.
The stated denominators and intervals are internally coherent and the document
appropriately distinguishes F1, tool selections, and token accounting, but
they cannot be independently recomputed from the cited repository evidence.

**Why it matters:** The reported positive paired localization deltas are
plausible evidence, not auditable evidence in this checkout. This limitation
does not undermine the direct functional tests, and the document correctly
refuses to infer patch correctness from localization.

### L2. The supply-chain audit cites mutable upstream `main`, not the pinned release source

**References:** `docs/supply-chain-audit.md:15-19,58-82` and
`pyproject.toml:20-31`.

**Expected behavior:** An audit used to justify loading downloaded native code
should identify the immutable source revision or release artifact actually
audited.

**Observed behavior:** The package pins `tree-sitter-language-pack==1.14.3`,
but the audit says it read `crates/ts-pack-core/src/download.rs` on upstream
`main`. It gives no commit hash or immutable source link tying the reviewed
download logic to 1.14.3.

**Why it matters:** Exact dependency pinning protects installation, but it does
not make the audit trail reproducible. The documented residual risk is candid:
the HTTPS manifest is not signed, so its digest is not an independent trust
anchor. Pin the audited source revision and record the release artifact hashes.

## 4. Research assessment

### Well supported

The overall localization, repair, and validation decomposition is grounded in
the [Agentless paper](https://arxiv.org/abs/2407.01489) and its
[canonical implementation](https://github.com/OpenAutoCoder/Agentless). This
supports the workflow's architectural plausibility and the project's derived
SEARCH/REPLACE and candidate-filtering machinery. It does not transfer
Agentless's benchmark results to this implementation.

Tree-sitter is an appropriate engine for concrete syntax trees and resilient
parsing. Official documentation explicitly describes incremental parsing and
recoverable `ERROR`/`MISSING` nodes. It does not provide name binding, type
resolution, or semantic correctness. The project's evidence tiers are a sound
way to expose that limitation rather than disguise it.

PageRank and modularity optimization are established graph techniques. Aider
also provides direct implementation precedent for combining Tree-sitter tags,
a file dependency graph, PageRank, and a token-budgeted repository map. These
sources support the mechanics, not the claim that this project's specific
edge weights, evidence multipliers, or community threshold maximize agent
success.

The MCP implementation aligns with the protocol's tool schema and annotation
model and with FastMCP's signature-driven validation. More importantly, the
security design does not trust annotations or client roots as enforcement: it
publishes no write handlers and applies an independent server-side allowlist.

### Weakly supported or still unproven

The four evidence tiers, common-name damping constants, community labels, and
shared-caller ranking are reasonable retrieval heuristics with good unit
coverage. There is no external comparative evaluation isolating those design
choices. Community modularity should be read as an orientation hint, not a
recovered architecture.

The 60-task results, as reported, support a localization advantage for both
structural-tool conditions over grep-only. They do not show a measurable
free-choice advantage over tool-only because that paired interval crosses
zero. The missing immutable raw artifacts reduce confidence in independent
reproduction, not necessarily in the arithmetic.

Downstream effectiveness remains unproven. Localization F1 is not patch
correctness, and the repository correctly says a controlled, test-verified
resolution experiment is still required. No claim about resolve rate, cost per
resolved task, or voting improving correctness should be made before that
experiment.

## 5. Firsthand testing observations

### Repository and automated-suite baseline

Measured command:

```bash
git status --short
uv run pytest -p no:cacheprovider
```

The initial status was empty. Pytest collected 1,483 tests: 1,436 passed and
47 skipped in 22.44 seconds. This is supporting evidence, not a substitute for
the interface tests below.

### Attached MCP surface

The connected server reported `agentless-mcp 0.3.0`, grammar pack 1.14.3, the
single configured repository root, no SQLite tag cache, and a clean receipt at
`02c15e20`. Live enumeration returned exactly:

```text
analyze_structure
capabilities
expand_symbols
explain_symbol
find_referencing_symbols
find_symbol
get_symbols_overview
list_dir
read_slice
repo_map
resolve_locations
```

Every listed tool was invoked. Representative exact invocations were:

```json
{"repo_root":"/home/dallasmarlow/Documents/Development/python/agentless-mcp","focus":["build_server"],"budget":1200,"max_files":6,"granularity":"function","no_cache":true}
{"repo_root":"/home/dallasmarlow/Documents/Development/python/agentless-mcp","stable_ids":["py:src/agentless_mcp/adapters/mcp/server.py::build_server"],"limit":2,"no_cache":true}
{"repo_root":"/home/dallasmarlow/Documents/Development/python/agentless-mcp","operation":"path","source":"py:src/agentless_mcp/adapters/mcp/server.py::serve","target":"py:src/agentless_mcp/adapters/mcp/server.py::build_server","include_unique":false,"include_ambiguous":false,"no_cache":true}
{"repo_root":"/home/dallasmarlow/Documents/Development/python/agentless-mcp","operation":"diagram","focus":"server.py","max_nodes":8,"group_by_communities":true,"no_cache":true}
```

The meaningful refusal cases were:

```json
{"repo_root":"/tmp","depth":1,"max_entries":10}
{"repo_root":"/home/dallasmarlow/Documents/Development/python/agentless-mcp","path":"../../../../etc/passwd","lines":[[1,2]],"context_lines":0}
{"repo_root":"/home/dallasmarlow/Documents/Development/python/agentless-mcp","depth":999,"max_entries":10}
```

They respectively produced an authorized-root refusal, an outside-root path
refusal, and a schema error naming the maximum depth of 20. Repeating the
focused map returned byte-identical output. Omitting `repo_root` selected the
sole configured root.

### CLI/MCP parity and annotations

Measured CLI command:

```bash
uv run agentless-mcp find-symbol build_server \
  --repo /home/dallasmarlow/Documents/Development/python/agentless-mcp \
  --limit 12 --no-cache
```

Its content matched the attached MCP `find_symbol` result; the only byte
difference was one trailing newline at the transport boundary. A separate
in-memory FastMCP probe against the disposable fixture reported eleven tools,
all four read-only annotation fields correct, structured content equal to the
text wrapper, unauthorized-root refusal, and byte-identical CLI/MCP map text:

```bash
uv run --extra mcp python /tmp/agentless-mcp-assessment.Ecj3WN/mcp_probe.py
```

### Cache and dirty working tree

The disposable repository was indexed outside its working tree and then had
`dirty_only` added to `core.py`. With the existing index and warmed grammar
cache, the ordinary cached query found the dirty-only symbol:

```bash
UV_CACHE_DIR=/tmp/agentless-assess-uv-cache \
XDG_CACHE_HOME=/tmp/agentless-mcp-assessment.Ecj3WN/xdg \
TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=/home/dallasmarlow/.cache/tree-sitter-language-pack/v1.14.3/libs \
uv run agentless-mcp find-symbol dirty_only \
  --repo /tmp/agentless-mcp-assessment.Ecj3WN/repo
```

The receipt named five dirty paths and a fresh tree-generation cache. The
result was `[py:core.py::dirty_only] @17-18`, showing per-file content
invalidation rather than stale-row reuse.

### Patch parsing, linting, validation, and refusal

Measured commands:

```bash
uv run agentless-mcp patch parse \
  -f /tmp/agentless-mcp-assessment.Ecj3WN/repo/candidates/01-fix-label.patch

uv run agentless-mcp patch check \
  -f /tmp/agentless-mcp-assessment.Ecj3WN/repo/candidates/01-fix-label.patch \
  --repo /tmp/agentless-mcp-assessment.Ecj3WN/repo

uv run agentless-mcp lint \
  --candidates /tmp/agentless-mcp-assessment.Ecj3WN/repo/candidates \
  --repo /tmp/agentless-mcp-assessment.Ecj3WN/repo --no-cache

uv run agentless-mcp validate \
  --candidates /tmp/agentless-mcp-assessment.Ecj3WN/repo/candidates \
  --repo /tmp/agentless-mcp-assessment.Ecj3WN/repo \
  --test-cmd "python test_regression.py" \
  --repro-cmd "python repro.py" --timeout 5 --jobs 1
```

The parser produced one edit and no errors. Syntax checking reported zero
Tree-sitter errors before and after. Lint explicitly reported that the
dependency-manifest check was not run rather than silently passing it.
Validation established a green baseline, a valid failing reproduction, and a
candidate that passed both regression and reproduction in a throwaway
worktree.

Running validation without `--test-cmd` refused the command from the fixture's
`.agentless-mcp.json` and named `--allow-repo-test-cmd` as the opt-in. The
timeout probe described in finding M1 returned `UNVERIFIED` and did not
evaluate the candidate.

### Standalone stdio observation

Measured probes:

```bash
timeout --signal=TERM --kill-after=2s 12s \
  env UV_CACHE_DIR=/tmp/agentless-assess-uv-cache \
  uv run --extra mcp python /tmp/agentless-mcp-assessment.Ecj3WN/stdio_probe.py

timeout --signal=TERM --kill-after=2s 12s \
  env UV_CACHE_DIR=/tmp/agentless-assess-uv-cache \
  uv run --extra mcp python /tmp/agentless-mcp-assessment.Ecj3WN/minimal_probe.py
```

Both servers started and logged the stdio transport, but neither client
completed initialization before the outer timeout. Because the minimal server
failed identically, this is an integration observation and missing test gate,
not a confirmed defect in `agentless-mcp`.

## 6. Final judgment

The repository's major functionality works in the present Linux environment.
The attached MCP integration, read services, graph views, stable-ID escalation,
root/path confinement, current-tree receipts, dirty-file cache invalidation,
patch parsing, and isolated validation were all observed directly.

The strongest genuinely enforced guarantees are:

- the MCP surface contains only eleven read operations;
- configured roots cannot be widened by ordinary client input;
- explicit paths are resolved and confined, including symlinks;
- repository test commands require explicit authorization;
- validation uses HEAD worktrees, a closed stdin, no shell, an environment
  allowlist, bounded output capture, and timeout-as-failure classification;
- cached structural rows are gated by file SHA-256;
- answers carry repository/generation receipts and output caps;
- grammar fetches happen only at explicit warmup or in the disable-able,
  digest-verified background warm at process start, never inside a tool
  call, and `AGENTLESS_MCP_NO_DOWNLOAD` forbids them entirely.

Claims that remain documentation- or evidence-dependent are the absolute
quality of the four reference tiers, community partitions as architecture,
the published 60-task localization gains until immutable raw artifacts are
linked, cross-platform behavior on Windows, and any downstream patch-quality
or cost-per-resolution improvement.

I would recommend the package for controlled real-agent use on Linux or macOS,
especially its read-only navigation surface and explicit CLI validation, with
four operating conditions: warm grammars before use, supervise validation with
allowance for termination grace, provide test commands explicitly, and do not
treat structural resolution as language-server ground truth. I would not yet
use the standalone console stdio command as an unmonitored production
dependency without an end-to-end transport gate.

The smallest changes or evidence needed to improve the verdict are:

1. Surface `RepoScan.skipped` reasons in symbol and map text responses and add
   cold-grammar regression tests through both adapters.
2. Either make `--timeout` a true wall-clock bound or document the termination
   grace explicitly and account for it in `--run-timeout`.
3. Add a CI test that spawns the installed `agentless-mcp-server`, initializes
   over stdio, lists the exact tool set, and executes one bounded call.
4. Publish an immutable `swe-explore-bench` revision plus raw task records,
   proof logs, and a recomputation script; pin the supply-chain audit to the
   exact upstream source revision reviewed.
5. Complete the already-identified downstream experiment before making any
   patch-correctness or cost-per-resolution claim.

The unifying result is that the architecture and most runtime guarantees are
real, while the remaining issues are concentrated at degradation reporting,
deadline semantics, transport integration coverage, and evidentiary
reproducibility rather than in the core structural machinery.
