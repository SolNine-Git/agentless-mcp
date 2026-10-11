# Rust Port, Phase 4: The MCP Door and Retirement

**Date**: 2026-10-10 | **Status**: not started | **Overview**: [2026-10-10_rust-port-00-overview.md](2026-10-10_rust-port-00-overview.md) | **Depends on**: P3 released | **Branch**: `feat/rust-port`, then `chore/retire-python` | **Est. Rust lines**: about 800, plus the gate report

## Goal

Serve the v2 MCP surface from the Rust binary on rmcp. Move the search-gate hooks and the erosion gate report into the binary, and move the erosion hook onto the Rust store. Ship wheels that keep `uvx agentless-mcp` working. After the release is live, delete the Python tree in a follow-up pull request. P4 passes when:

- the published tool list equals the 0.10.1 v2 surface;
- the installed wheel serves a real stdio and HTTP session;
- the release follows AGENTS.md.

## Scope

- **In**: the `mcp` crate. `hook` gains the search-gate commands, and the erosion hook moves from the `tags.db` reader to the Rust store. `gate-report` is a new subcommand. The `agentless` binary gains the server door. Wheels and release CI are added.
- **After the release** (`chore/retire-python`): the Python source is deleted.
- **Out**: the v1 surface (decision 2). The binary refuses it at start-up.

## Work items, in build order

### 1. `mcp`

- **Server**: `Server` implements rmcp's `ServerHandler`. The rmcp version is pinned to one major; each major upgrade is a reviewed change.
- **Tools**: `list_tools` and `call_tool` come from `app::OPS`:
  - `orient`, `symbols`, and `read`, each with its operation table (`src/agentless_mcp/adapters/mcp/server.py:1509-1877`);
  - the shared tools `find_referencing_symbols`, `history`, and `capabilities` (server.py:1452-1505).
- **Wire contract, kept exactly**:
  - Tool and parameter names, descriptions from `prompts`, and the accepted and required order of each operation table.
  - Read-only annotations with their titles (`src/agentless_mcp/adapters/mcp/annotations.py`).
  - `_meta {"anthropic/alwaysLoad": true}` on every v2 tool except `history` (server.py:1049-1055).
  - Text-only results.
  - The rule that a zero or empty value counts as absent (server.py:1564-1585).
  - The refusal templates.
- **Repository selection**: `select_root(allowed, requested, advertised, launch)` is a pure function (server.py:426-454, 537-610).
  - The allowlist is the `--root` flags, fixed for the process, and the `--roots-from` files. A roots file is read again when its (mtime_ns, size) changes, as `RootsFile.current` does (`src/agentless_mcp/adapters/mcp/cliargs.py:113-160`; server.py:12-24, 522-535). So a line added or removed takes effect on the next call, without a restart.
  - Client roots come from `peer.list_roots()` with the 2 s timeout (server.py:899-925). A client without roots support is told apart from a failure.
  - rmcp marks `Peer::list_roots` deprecated since 1.8.0 (SEP-2577). The one call site needs an entry in `crates/suppressions.toml` (overview rule 4), which the owner approves, until rmcp offers the replacement.
- **Call limits**:
  - one `Deadline` per call, with the same limit and grace as today;
  - the concurrency gate;
  - offloading of blocking work to a blocking pool;
  - the unplanned-error text (server.py:1081-1084).
- **Transports**: stdio (the default) and streamable HTTP. The server flags and defaults come from `src/agentless_mcp/adapters/mcp/cliargs.py:394-519`.
- **Auto-index**: the `AutoIndexer` from P2 fills the Rust store on first use of each root, off the calling thread.
- **Restart on upgrade**, for the HTTP transport only, as today (server.py:1905-1925). A stdio process ends with its client and loads the new binary on reconnect.
  - At start-up, resolve the installed binary path once and store it. The fingerprint is (device, inode, mtime) of that path, polled. It is not taken from `std::env::current_exe()`: on Linux, after the file is replaced, that returns `"<path> (deleted)"`, and `metadata` on it fails (measured with rustc on this machine).
  - A changed fingerprint triggers a restart only after 60 s of uptime (`src/agentless_mcp/core/selfrestart.py:92`). An absent fingerprint mid-upgrade never triggers, as today (`selfrestart.py:117-162`).
  - Then the server cancels its root token, drains in-flight calls, and execs the stored path with the same argv (finding E10). There is no self-signal and no interrupt flag.
- **`--surface v1` and `--surface both`**: refused at start-up with exit 2 and a message that names the v1-to-v2 mapping table in the agent guide.
- **Tests**:
  - In-process duplex transport, with no listening port: every tool and operation, roots present, roots absent, roots timing out, a roots file edited between two calls, the deadline with an injected clock, the concurrency gate, and the refusal templates.
  - A `tools/list` snapshot.
  - Restart tests with a fake fingerprint source: no restart under 60 s of uptime, none on an absent fingerprint, none under stdio.

### 2. `hook`: search gate, and the erosion hook's cache

- `agentless-mcp hook gate-check` (PreToolUse on Grep, Glob, and Bash) and `agentless-mcp hook gate-mark` (PostToolUse on `mcp__agentless__.*`) replace `contrib/hooks/agentless_gate_check.py` (362 lines) and `contrib/hooks/agentless_gate_mark.py` (181 lines).
- The names of the operations that unlock the gate come from `OpSpec` (a `localizes` flag). There is no hand-kept copy. One hook crate holds the shared log and marker helpers (finding F14).
- The marker file location and format stay the same (`marker_path`, `agentless_gate_mark.py:95`), so a session can move between builds.
- **The erosion hook moves to the Rust store**, which the Rust MCP server fills. The `tags.db` reader (P1, decision 9) is deleted. A missing or empty Rust store gives the `no_tag_cache` gap, as before.
- **Tests**: the shell-text cases of `tests/unit/test_gate_hooks.py` as a table; the marker written after a localizing call and not after another call; the erosion hook outcome suite again, on the Rust store, fresh, stale, and absent.

### 3. `gate-report`

- `agentless-mcp gate-report` replaces `contrib/hooks/erosion_gate_report.py` (889 lines). It:
  - reads the JSONL log and the transcripts;
  - classifies each finding as not applied, unresolved, unobserved, acted on, or not acted on;
  - prints each check's effective false-positive rate;
  - writes and scores the labeling CSV;
  - prints the checks that meet the advisory-to-block criterion (50 applied findings and a labeled precision of 90% or more).
- Log records and the CSV use the same schema as today, so a trial that started under Python continues.
- **Tests**: the cases of `tests/unit/test_erosion_gate_report.py`, as fixtures.

### 4. Wheels and release CI

- **Packaging**: `pyproject.toml` switches the build backend to maturin with `bindings = "bin"`. The version comes from the workspace `Cargo.toml`. maturin parses it as a PEP 440 version (`pep440_rs`, maturin `src/metadata.rs:653`), so `0.11.0-rc.1` becomes `0.11.0rc1` (overview decision 10).
- **Commands**: the wheel installs two console commands. `agentless-mcp` is the main binary. `agentless-mcp-server` is a second, thin `[[bin]]` target that starts the server door with the same flags. Packaging every bin target was fixed in maturin in 2022 (maturin#947); a spike builds the wheel and lists its contents before this item starts.
- **Targets**: Linux x86_64 and aarch64 (manylinux), and macOS arm64 and x86_64. An sdist is published too, and it builds with a Rust toolchain and the vendored grammar sources (P0 D1).
- **Notices**: the grammar licenses from the pinned grammar sources (P0 D1), and `cargo about` for the other crates. The wheel and the release archive include them.
- **CI**: one release job builds every wheel. It then installs each wheel in a clean environment, runs `agentless-mcp --version` and one `map` call, and uploads to PyPI only after all pass.

### 5. Release

The release goes out in two steps (overview decision 10). Each step follows the procedure below, with X set to that step's version.

1. **`0.11.0rc1`** (Cargo `0.11.0-rc.1`): a PyPI pre-release. Plain `uvx agentless-mcp` and `uv tool install agentless-mcp` skip it and keep Python 0.10.1. The three deployed surfaces opt in with `agentless-mcp==0.11.0rc1` for the trial. A defect found in the trial ships as `0.11.0rc2`, and so on.
2. **`0.11.0`**: when the P4 exit gates pass and the trial is clean. Plain installs then get the Rust build.

`1.0.0` is not part of P4. It waits for more validation after `0.11.0`.

- Before "Prepare X": run the Python black-box harness (the swapped-driver tests from P2 and P3) against the release candidate. The diff must be empty.
- Follow AGENTS.md:
  1. "Prepare X": the changelog entry, the version in `Cargo.toml` and `pyproject.toml`, `Cargo.lock`, and `uv.lock` while it exists.
  2. "Release X: date the changelog", after CI is green.
  3. A merge commit, then the tag `vX`, then the PyPI upload of every wheel.
- CHANGELOG.md:
  - **Removed**: the v1 surface and `--surface`.
  - **Changed**: the implementation is Rust, with measured hook, map, and refs latency against 0.10.1. The cache file is new; old `tags.db` files under the cache directory are no longer read and can be deleted.
- Run the release regression gate against 0.10.1:
  - the suites;
  - the Loc-Bench retrieval tier byte diff;
  - the surface diff;
  - the exposure check before any paid run.
- After each upload, update the three deployed surfaces and check each running process, not its receipt:
  - the global tool (`uv tool install agentless-mcp==X`, which also selects a pre-release);
  - the long-running HTTP server on port 8766;
  - the copies of the hooks in `~/.claude/hooks`, which now call `agentless-mcp hook ...`.

### 6. After the release: retire the Python tree (`chore/retire-python`)

The Python tree stays in the repository, unpackaged, until the release is live and the three deployed surfaces run it. Then one pull request deletes it, in this order:

1. Move `src/agentless_mcp/prompts/*.json` into `crates/prompts/data/` and `src/agentless_mcp/docs/agent-guide.md` into `crates/app/data/`. Point `cargo xtask generate` and `include_str!` at the new paths.
2. Delete:
   - `src/agentless_mcp/`;
   - `tests/unit/` and the Python parts of `tests/characterization/`, including each module's `regenerate()`; `cargo xtask goldens --update` replaces them (overview rule 6);
   - `contrib/hooks/*.py`;
   - the Python fixture generators `scripts/pycompat_fixtures.py`, `scripts/pair_parity_fixtures.py`, `scripts/facts_parity_fixtures.py`, `scripts/patch_parity_fixtures.py`, and `scripts/scb_parity_goldens.py`. Their committed fixtures stay as data;
   - the Python-only CI jobs (`ci.yml` fresh-install, no-extras, wheel data);
   - the Python lint configuration with no files left to check: the ruff, mypy, import-linter, and deptry hooks in `.pre-commit-config.yaml`, and their sections in `pyproject.toml`. `codespell` stays, because it also checks the docs and the Rust code.
3. Keep `tests/characterization/fixtures/` and `goldens/`. The Rust tests read them byte for byte.
4. Rewrite AGENTS.md: the commands (`cargo nextest run`, `cargo xtask layers`, `cargo xtask erosion`), where things live (the crate layer table), and the release steps for maturin. The benchmark methodology stays.
5. Update the README install and the hook settings (`contrib/hooks/settings-example.json` calls `agentless-mcp hook ...`). Update `docs/skills/agentless-mcp/SKILL.md`.

## Exit gates

- [ ] The `tools/list` snapshot equals the 0.10.1 v2 surface: names, parameters, descriptions, annotations, and `_meta`. The comparison is generated from both builds, not hand-written.
- [ ] In-process transport tests pass: every operation, roots (including a roots file edited between calls), deadline, concurrency, refusals, and restart.
- [ ] The installed wheel serves one stdio session and one HTTP session. This runs in a separate CI job, marked as end-to-end, because it binds a port.
- [ ] The search-gate and gate-report tests pass, and a Python-era gate log reads correctly.
- [ ] The erosion hook outcome suite passes on the Rust store, and the P1 latency gate holds on it in each cache state.
- [ ] The Python black-box harness shows an empty diff before "Prepare X".
- [ ] The release regression gate passes, and the three deployed surfaces run the new version.
- [ ] After the follow-up pull request: the Python tree is gone, and CI is green with the Rust jobs only.

## Risks

| Risk | Response |
|---|---|
| rmcp changes its major version during P4 | The pin holds. Upgrade in a separate commit, with the `tools/list` snapshot as the check. |
| A client depends on pydantic's exact schema bytes (titles, the anyOf-null form) | Compare the generated `tools/list` with 0.10.1 byte for byte. If a difference remains, document it under Changed with the field named. |
| A user still pins v1 | The 0.10.1 deprecation notice names this release. The start-up refusal names the mapping table. |
| The MCP spec deprecates roots (SEP-2577), and rmcp deprecates `list_roots` | The wire behavior stays for at least one year. Keep `list_roots` under its one approved suppression and track the replacement in a separate change. |
| The wheel for one target fails to build | The release job uploads nothing until every target builds and passes the install test. |
| The release has a defect found after the upload | The Python tree is still in the repository until the follow-up pull request, so a fix can compare both builds. |

## Version (owner, 2026-10-10)

The Rust release is `0.11.0rc1`, then `0.11.0` (overview decision 10). Below 1.0, SemVer allows the removal of v1 in a minor release. `1.0.0` comes after more validation.
