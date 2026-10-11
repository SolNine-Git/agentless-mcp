# Rust Port, Phase 0: Groundwork

**Date**: 2026-10-10 | **Status**: P0a merged (PR #60, `9bb51ac`); P0b (`0037919`) and P0d committed on `feat/rust-port`; P0c next | **Overview**: [2026-10-10_rust-port-00-overview.md](2026-10-10_rust-port-00-overview.md) | **Depends on**: nothing | **Est. Rust lines**: about 600

## Goal

Make the Python build correct before it becomes the parity base, release it, and stand up the Rust workspace with every gate proven on a negative fixture. P0 has four parts, each on its own branch or step.

`feat/erosion-checks` merged as PR #59 (`4ec78ac`, from base `d7f0c52`) and was tagged `v0.10.0` before P0a and P0b. So 0.10.0 holds neither, and the parity base is 0.10.1. P0a and P0b each start from `main`.

| Part | Branch | Content |
|---|---|---|
| P0a | `fix/erosion-followups` | Fixes to the erosion features that 0.10.0 shipped. |
| P0b | `feat/rust-port` | Defects that predate this branch, the approved deprecations, and the grammar-pin decision (D1 runs before P0b merges). |
| P0c | release | Python 0.10.1, the parity base and the gate binary. |
| P0d | `feat/rust-port` | The workspace, `kernel`, CI, and the gates. |

## P0a: Erosion-check fixes (`fix/erosion-followups`)

### A1. One path-scope rule (decision 8)

- `src/agentless_mcp/application/symbol_service.py:108`: add `testdata` to `FIXTURE_DIRECTORY_SEGMENTS`.
- `src/agentless_mcp/application/lint_service.py:152-156` (`_complexity_delta`): drop pairs whose path is a fixture path. Keep test paths.
- `src/agentless_mcp/application/lint_service.py:159-160` (`_contract_drift`): drop test and fixture pairs, the rule `health` uses (`graph_service._in_health_scope`). Not `in_metric_scope`, which also tests tier-1: decision 8 is about paths only.
- The hook form and `--since` share `_run_pairs` (`lint_service.py:959`), so both are covered. The one constant also covers `health` and the clone index.
- Tests, in `tests/unit/test_lint_service.py`:
  - A fixture pair gives no `complexity_delta` finding.
  - A test-file pair keeps its `complexity_delta` finding.
  - A test-file pair under a boundary glob gives no `contract_drift` finding.
  - A `pkg/testdata/x.go` file is outside `health` and the clone index.
- Predicted golden diff: none. The lint goldens run on fixture repositories, where paths are relative to the fixture root and hold no `fixtures` segment. Run the goldens to confirm.
- Re-run `lint --since d7f0c52`, the base of the erosion-checks work: the warning on `tests/characterization/fixtures/complexity_py/branches.py:68` (finding S2) goes away.

### A2. The hook reads config with lint's rules (defect, finding F13)

- `contrib/hooks/agentless_erosion_gate.py:402-418` (`_config_document`) opens `.agentless-mcp.json` with no checks. Lint refuses a config that resolves outside the repository or is not a regular file (`src/agentless_mcp/core/projectconfig.py:189-213`).
- Change: open with `os.open(path, O_RDONLY | O_NONBLOCK)`, then `os.fstat`; refuse anything but a regular file; refuse a resolved path outside the resolved root. Use the same refusal wording as `projectconfig`. The hook stays standard-library only.
- `O_NONBLOCK` with `fstat` removes the FIFO stall and the check-then-open race in one step.
- Tests, in `tests/unit/test_erosion_gate_hook.py`:
  - A symlink to a config outside the repository: ignored, with a warning.
  - A FIFO at the config path: ignored, and the hook does not block. Use a per-test timeout, not a sleep.
  - A symlink to a config inside the repository: read, as lint reads it.
  - Extend `TestTheHookAgreesWithThePackage` from constants to behavior. For each case above, the hook's decision equals `projectconfig.load`'s decision.

### A3. A failing exit for gates (`--fail-on`)

- Today `lint` exits 0 on any run that happened (`src/agentless_mcp/adapters/cli/main.py:1341-1357`), so CI cannot block on a finding.
- Add `--fail-on SEVERITY[,SEVERITY...]` to the `lint` parser (main.py:604-659). The values are `warning`, `advisory`, and `not_checked`. An unknown value is a usage error (exit 2).
- Exit 3 when any finding has a listed severity. Exit 1 keeps its meaning, an operation failure, so CI can tell a broken tool from a failed gate. Without the flag, lint still exits 0 on a run that happened.
- Change the text that promises exit 0: the parser description (main.py:609-613) and the `_cmd_lint` docstring (main.py:1342-1346).
- Add exit 3 to the exit-code table in `src/agentless_mcp/adapters/cli/formatting.py:42-44`, the agent guide, and the README.
- Tests, in `tests/unit/test_cli.py`: exit 3 on a finding with the flag; exit 0 without the flag; exit 2 on an unknown severity; `not_checked` alone.

### A4. Records

- CHANGELOG.md, Unreleased:
  - **Changed**: A1, with the rule stated per check.
  - **Fixed**: A2.
  - **Added**: A3.
- Agent guide: the lint section states the scope rule and exit 3.

### P0a exit gate

- `uv run pytest -q` and `uv run pre-commit run --all-files` pass.
- Every golden diff was predicted; A1 to A3 predict none.
- `lint --since d7f0c52` shows no fixture warning.

## P0b: Pre-port defects and deprecations (`feat/rust-port`)

Before each item, write the predicted golden diff into the pull request description: list the golden files and the lines expected to change. Regenerate with each module's `regenerate()` only after the prediction is written. A diff that was not predicted stops the item.

### B1. Raw docstrings keep their quotes (decision 3)

- `src/agentless_mcp/core/extractor.py:4140-4147` strips three characters only when the text starts with the quote. A prefixed literal such as `r"""Doc."""` keeps `"""Doc.""`.
- Change: remove the string prefix before the delimiter test, with the existing `string_prefix` rule. `normalize.py:70` imports `extractor`, so `extractor` cannot import `normalize`. Move `string_prefix` (`normalize.py:442-445`) into `extractor.py` and import it from there in `normalize.py` and `complexity.py` (its callers at `normalize.py:487` and `complexity.py:352`).
- Tests: `r`, `u`, `b`, `f`, `rb`, and `Rb` prefixes, each with single and triple quotes.
- Prediction: list every fixture docstring with a prefix (`grep -rnE '^\s*[rRbBuUfF]{1,2}("""|'"'"''"'"''"'"')' tests/characterization/fixtures`); each one's skeleton and card line changes.

### B2. Class kind compares base names (decision 3)

- `extractor.py:3597-3606` tests substrings, so `Bar(ProtocolError)` is a protocol and `Foo(Enumerable)` is an enum (verified).
- Change: compare `symbols.base_name(base)` exactly. Protocol: `Protocol`. Enum: `Enum`, `IntEnum`, `StrEnum`. The set stays as it is; adding `Flag` or `IntFlag` is a separate decision.
- Tests: `ProtocolError`, `Enumerable`, `typing.Protocol`, `Protocol[T]`, `enum.Enum`, `enum.IntEnum`.
- Prediction: fixtures whose base names contain `Protocol` or `Enum` without being equal to them.

### B3. One owner name for a Rust impl (decision 3)

- `extractor.py:3812-3813` names the owner with the whole type text (`Foo<T>`). `src/agentless_mcp/core/complexity.py:408-412` takes the first `type_identifier` (`Foo`).
- Change: one function in the extractor computes the owner from the impl's `type` field: the first `type_identifier`, else the first `primitive_type`, so `impl Trait for i32` keeps its owner `i32`. The complexity code imports it, which the one-parse layer contract permits (complexity sits above extractor).
- Tests: `impl<T> Foo<T>`, `impl Trait for Foo<T>`, `impl<'a> Foo<'a>`, `impl Trait for i32`, and a plain `impl Foo`. The extractor's owner and the complexity unit's owner agree in each case.
- Prediction: stable ids of methods in generic impls change from `Foo<T>.bar` to `Foo.bar`. List each such fixture method.

### B4. Java records set a scope (decision 3)

- The Java row (`extractor.py:403-414`) lacks `record_declaration` and `compact_constructor_declaration`; `complexity.py:623-640` has both.
- Change: add `record_declaration` to the class node types and `compact_constructor_declaration` to the function node types. Check that the record body shape is handled by the class extractor.
- Tests: in a nested record (`class A { record R(int x) { void m() {} } }`), `R` has parent `A`, and `m` and the compact constructor have parent `R`. The extractor names only the innermost type as the owner, for every nested Java class, while complexity names the chain (`A.R.m`). That difference predates B4 and stays open.
- Prediction: Java fixtures with records gain symbols.

### B1 to B4: cache versions

- B1 to B4 change stored rows without changing their shape, so `SCHEMA_VERSION` goes from 16 to 17 (`src/agentless_mcp/core/cache.py:185`). B3 changes the complexity unit of an `impl` on a primitive type, so `METRICS_ALGORITHM` goes from 1 to 2 (`src/agentless_mcp/core/fileparse.py:25`).
- Tests: a version 15 or 16 database is rebuilt, and a reader discards it.

### B5. Communities: delete the hash, show a partial grouping (decision 7)

- Delete from `src/agentless_mcp/core/communities.py`:
  - `HASH_VERSION`, `_HASH_SEPARATOR`, and `community_hash` (lines 100-110, 318-340);
  - `Community.member_hash`, `Community.as_dict`, and `CommunityPartition.as_dict` (lines 140-158, 208-217);
  - the `passes` field.
- Keep `converged` (`communities.py:198`). `src/agentless_mcp/application/graph_service.py:352-380` builds `render.CommunityReport` without it today. Pass it in.
- The text output adds one note when `converged` is false; put the message in `prompts/messages.json`, with a sample in `tests/unit/test_prompts.py`. The JSON output adds `"converged"`, which is an additive change.
- Tests: delete the hash tests (`tests/unit/test_communities.py:306-366`). Keep the convergence tests at lines 151-170. Add render tests for the note and for the JSON field, both with `converged` false.
- Prediction: no golden changes. The three `graph/*.communities.txt` goldens converge, so they show no note, and no communities JSON golden exists.

### B6. Deprecate v1 (decision 2)

- Add a start-up warning on stderr when `--surface` is `v1` or `both`. It names the removal in 0.11.0, the release that ships the Rust MCP server (overview decision 10), and the v1-to-v2 mapping table in the agent guide.
- `src/agentless_mcp/adapters/mcp/cliargs.py:457-467`: the help text says "deprecated".
- Replace "for one release" with the removal statement in:
  - `README.md:302-304`
  - `src/agentless_mcp/docs/agent-guide.md:159`
  - `docs/skills/agentless-mcp/SKILL.md:78`
- CHANGELOG.md: add a **Deprecated** section.
- Tests: the warning appears for `v1` and `both`, and not for `v2`.

### B7. Auto-index off the event loop, and no leaked connection (defect)

- `src/agentless_mcp/adapters/mcp/server.py:1216` calls `handlers.refresh_in_background` on the event loop. That call reaches `cache.start_auto_index` (`src/agentless_mcp/core/cache.py:1330`) and `_stale_generation` (1375-1394), which walks the repository's file stats in `repo_generation` and opens SQLite in `_index_current` (1458-1473), with a 5 s busy timeout.
- Change: `await offload(handlers.refresh_in_background, ctx, no_cache=no_cache)`.
- `cache.py:1021` computes `repo_generation` after the connection is open and outside any handler. Change: compute it inside a block that closes the connection on any exception.
- Tests:
  - With a fake `start_auto_index`, the call runs on a worker thread, not the loop thread.
  - With a fake `repo_generation` that raises `WalkBoundExceeded`, the connection is closed.

### B8. One rule for an unstatable file (defect)

- `src/agentless_mcp/util/fslimits.py:318-323` keeps an unstatable file with size 0. `src/agentless_mcp/core/treewalk.py:222-227` drops it.
- Change: drop it and report the reason, in both places, through one function.
- Tests: an unstatable entry gives the same result through both paths.

### B9. One version string (defect)

- `src/agentless_mcp/adapters/mcp/cliargs.py:42-55` falls back to `0+unknown`, and `src/agentless_mcp/application/capability_service.py:234-239` falls back to `unknown`.
- Change: one function in `util` owns the lookup and the fallback.

### B10. The community tie rule is stated wrongly (defect)

- The module docstring (`src/agentless_mcp/core/communities.py:47-52`) says that among candidates that clear the bar, the first in ascending community-id order wins. The loop (`communities.py:401-407`) keeps a running best: a later candidate replaces it when its score is higher by more than `EPSILON`.
- Change: correct the docstring to the loop's rule. The code does not change, and P2 ports the code's rule.
- Prediction: no golden changes.

### P0b exit gate

- Suites and pre-commit pass.
- Every golden diff matches its written prediction.
- `lint --since $(git merge-base HEAD main)` on the branch reports no new complexity or clone finding.

## P0c: Release 0.10.1

- Merge the P0a pull request, then the P0b pull request.
- Follow the AGENTS.md release gate:
  1. The suites, with every golden diff predicted.
  2. The Loc-Bench retrieval tier from a frozen copy, with `ranked_files` byte-diffed against the stored arm for the base commit.
  3. The surface diff.
  4. The exposure check.
- Two commits: "Prepare 0.10.1", then "Release 0.10.1: date the changelog". A merge commit, then the tag `v0.10.1`, then the PyPI upload.
- Install it as the gate binary: `uv tool install agentless-mcp==0.10.1`.
- Freeze a copy of the 0.10.1 build without `.git` and `.venv`. It is the parity base for every later phase.

## P0d: Workspace and gates (`feat/rust-port`)

### D1. Pin the grammar sources (runs before P0b merges)

- D1 is research with no code. It runs before P0b merges, because its fallback changes the Python pin, and 0.10.1 (P0c) freezes the parity base.
- The goldens come from tree-sitter-language-pack 1.14.3 (`pyproject.toml:35`, also pinned in `.pre-commit-config.yaml:66`). The Rust build must link the same grammar revisions, or parse trees can differ.
- Find the tree-sitter-language-pack crate version whose grammar sources equal the 1.14.3 manifest. Check that it links statically, which grammars it selects at build time, and whether its build script downloads sources. A build that downloads needs a vendored, checksummed source set, because CI and the sdist build offline.
- Compare the grammar revisions one by one against the Python wheel's. Write the list into the overview.
- If no crate version matches and links statically: in P0b, bump the Python pin (both files) and the crate together, with a predicted golden diff. Never bump one alone, as `pyproject.toml:27-33` says.
- The grammar licenses come from the pinned grammar sources. The Python wheel ships no notice file to copy.
- Expect the vendored path. On 2026-10-10 the crate's latest version is 1.21.3. Its default features are `download` and `dynamic-loading`, its build script depends on `ureq`, it declares no static-linking feature, and docs.rs fails to build it (the last successful docs.rs build is 1.9.0-rc.6).

### D2. Workspace root

- Add `Cargo.toml` with `[workspace]`, `members = ["crates/*", "xtask"]`, `[workspace.package]` (edition 2024, `rust-version` matching the pinned tree-sitter crate), and `[workspace.lints]`:
  - `unsafe_code = "forbid"`;
  - clippy `pedantic` at warn;
  - `allow_attributes = "deny"`, kept as a first warning; rule 4's xtask is the gate.
- Every crate but `syntax` sets `[lints] workspace = true`. Cargo refuses an override next to `workspace = true` (cargo#13157), and `forbid` cannot be lowered. So `syntax` writes its own `[lints]` table: the workspace table without `unsafe_code`, plus `unsafe_op_in_unsafe_fn` and `clippy::undocumented_unsafe_blocks` at deny. `cargo xtask lint-tables` checks that the two tables differ only in those entries, and that `unsafe` appears only in `crates/syntax/src/ffi.rs`.
- The erosion gate owns cyclomatic complexity, so clippy's `cognitive_complexity`, a different metric, stays off.
- `clippy.toml`: `too-many-lines-threshold = 60`, `too-many-arguments-threshold = 5`, `excessive-nesting-threshold = 4`, and `disallowed-methods` for the file writes: `std::fs::write`, `std::fs::rename`, `std::fs::copy`, `std::fs::create_dir_all`, `std::fs::remove_file`, `std::fs::remove_dir_all`, `std::fs::File::create`, `std::fs::OpenOptions::open`, and `rusqlite::Connection::open`.
- Clippy reads `clippy.toml` from the crate's own directory first. `crates/io/clippy.toml` repeats the root file without the `std::fs` entries, and `crates/store/clippy.toml` without the `rusqlite` entry. `cargo xtask lint-tables` also checks that each crate file equals the root file apart from those entries.
- `rust-toolchain.toml` pins one stable release. `cargo public-api` needs a nightly toolchain to build rustdoc JSON, so CI installs one pinned nightly with the minimal profile for that job only.
- Add `rustfmt.toml`.
- `deny.toml`: advisories, licenses (allow-list), bans, and sources (crates.io only).
- Exclude `crates/` and `target/` in `pyproject.toml` (ruff `exclude`, codespell `skip`, deptry). Add `target/` to `.gitignore`.

### D3. The `kernel` crate

- Public items:
  - `Error`, with thiserror, one variant per Python `AgentlessError` subclass (`src/agentless_mcp/util/errors.py`);
  - `Limit<const MIN: u32, const MAX: u32, const DEF: u32>`, whose `Deserialize` refuses a value out of range;
  - `Bounded<T> { rows, total, limit }` with an `omitted()` method;
  - `Key([u8; 32])`, written as hex on the wire;
  - `Span`, and the typed ids `FileId`, `NameId`, and `NodeId`.
- Tests: serde round trips; `Limit` refuses MIN-1 and MAX+1 and fills in DEF; `Bounded::omitted` is never negative.
- Commit a public API snapshot (`cargo public-api`).

### D4. The `xtask` crate (development tool, not one of the 19)

- `cargo xtask layers` reads `cargo metadata` and `crates/layers.toml` (crate name to layer). It fails when an edge points to the same or a higher layer, or when a crate has no row.
- `cargo xtask public-api` compares each crate's public API with its committed snapshot.
- `cargo xtask suppressions` and `cargo xtask lint-tables` (overview rule 4; D2).
- `cargo xtask goldens --update` rewrites golden files after a written prediction (overview rule 6).
- `cargo xtask erosion --lint-bin PATH --health-bin PATH [--base REV]` runs the pinned gate binaries over `crates/`. `--base` defaults to `git merge-base HEAD origin/main`.
  - First `index` with the lint binary, so `lint --since` reads the clone index from the cache and never stops at its 10 s budget (`src/agentless_mcp/util/deadline.py:26`).
  - `lint --since <base> --only complexity_delta,clone_delta,contract_drift --fail-on warning,advisory,not_checked`;
  - then `health --repo crates --json` with the health binary, compared with `crates/erosion-baseline.json`. It fails if erosion, units over threshold, or clone share rises. `--update` writes a lower value only.

### D5. Project config

- In `.agentless-mcp.json` at the repository root:
  - `boundary_globs`: `crates/io/**`, `crates/git/**`, `crates/store/**`, `crates/hook/**`, `crates/mcp/**`;
  - `erosion_gate`: `block` for `complexity_delta`, `contract_drift`, and `clone_delta`.
- Block mode also gates Python edits during dual maintenance, which is intended.

### D6. CI and pre-commit

- `.github/workflows/ci.yml` gains a `rust` job on `ubuntu-latest` and `macos-latest`. It runs:
  - `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo nextest run` with a per-test timeout, and `cargo insta test --check`;
  - `cargo deny check` and `cargo machete`;
  - `cargo xtask layers`, `public-api`, `suppressions`, `lint-tables`, and `erosion`.
- In P0, both gate binaries are Python 0.10.1 from `uv tool install agentless-mcp==0.10.1`. The hand-over to the Rust gate follows overview rule 2.
- `.pre-commit-config.yaml` adds `cargo fmt` and `cargo clippy` for `crates/`.

### D7. Negative fixtures for every gate

- `scripts/gate-selftest.sh` copies the workspace into a temporary git repository with no remote. It commits a base, applies one violation at a time, and asserts that the matching gate fails. It passes `--base` to `xtask erosion`, because the copy has no `origin/main`. The violations:
  1. A function above complexity 10 (`complexity_delta`, exit 3).
  2. A copied block of 40 or more tokens (`clone_delta`, exit 3).
  3. `unwrap_or_default()` on a parsed value in `crates/io` (`contract_drift`, exit 3).
  4. A file that does not parse (`not_checked`, exit 3).
  5. `#[expect(clippy::too_many_lines)]`, and separately `#![allow(...)]` (`xtask suppressions`).
  6. An upward crate edge (`xtask layers`).
  7. A new public function in `kernel` (`xtask public-api`).
  8. A health value above the baseline (`xtask erosion`).
  9. `std::fs::write` in a crate other than `io` (clippy `disallowed_methods`).
  10. `unsafe` outside `crates/syntax/src/ffi.rs`, and a `syntax` lint table that drifts from the workspace table (`xtask lint-tables`).
  11. A dependency with a license outside the allow-list, and one from a git source (`cargo deny`).
  12. An unused dependency (`cargo machete`).
  13. A changed insta snapshot that was not accepted (`cargo insta test --check`).
  14. A changed golden file (the byte comparison of overview rule 6).
- CI runs the self-test.

### D8. The latency script and the design records

- `scripts/hook_latency.py` commits the overview's baseline method: a frozen copy without `.git` and `.venv`, the mcp-local `f17af96` Write payload, 30 runs per cache state, and an isolated `XDG_CACHE_HOME`. It measures the four baseline rows for `1dc4d70` and for 0.10.1, and the overview table then cites the script's runs. P1 scores its latency gate with the same script.
- Copy the design page's records into `docs/analysis/`: the findings (A1-F15, S1-S2), the Codex findings (V1-V47) and their resolutions, the parity ledger, and the measured alternatives. The phase plans cite those ids, so they must resolve inside the repository before P1 starts.

### P0d as built

Where the build differs from D2 to D8:

- **Excludes**: `crates/` needs none in `pyproject.toml`, because ruff and deptry read Python only. Codespell checks the Rust sources on purpose. Its `skip` in `pyproject.toml` lists `target`, because a bare `codespell` walk does not read `.gitignore`. The `skip` also lists the generated `public-api.txt` snapshots, where serde's module paths read as typos, and the pre-commit hook excludes them too.
- **`rust-version`** is 1.94, the pinned toolchain, until P1 adds the tree-sitter crate (D2).
- **Version**: the workspace version is `0.11.0-rc.1`, the first Rust release (overview decision 10), and no crate publishes (`publish = false`). Until P4 ships the wheel, the PyPI release from `pyproject.toml` is the product version.
- **Per-crate `clippy.toml`**: the `io` and `store` files arrive with those crates in P1, and `cargo xtask lint-tables` checks each one that exists. `xtask/clippy.toml` drops the `std::fs` entries, because xtask writes the baseline and the snapshots.
- **Static gates**: `cargo xtask static-gates` runs `layers`, `suppressions`, and `lint-tables`, reports all three, and fails if one fails. Pre-commit and CI run it, and each of the three still runs alone. `cargo xtask nightly` prints the pinned nightly, and CI installs that toolchain for `public-api`.
- **What the static gates read**:
  - Tool config: `lint-tables` refuses each clippy, rustfmt, rust-toolchain, and cargo-deny config file, each `.agentless-mcp.json`, and each `.cargo/` path in the repository, with names compared without case. It allows the root files and the `clippy.toml` of `crates/io`, `crates/store`, and `xtask`. The root `.cargo/config.toml` holds only the `xtask` alias. For each package, the nearest `clippy.toml` or `.clippy.toml` at or above its directory must be the root file, unless the package directory is one of those three. That check reads the disk, so an ignored clippy file fails it. The other checks list files through git's ignore rules.
  - Source scope: the gates take the packages from `cargo metadata` and tokenize every file under `crates/` and `xtask/` whose extension is `.rs` in any case. They refuse these forms, each with its file and line:
    - Files: a `.rs` extension in another case, a symlink, a package outside `crates/` and `xtask/`, and a target whose source is not a `.rs` file inside its package.
    - Literal routes to another file: `#[path]`, `include!`, a use item that names `include`, and the identifier `include` anywhere.
    - Macro routes: a `#` that does not start `#[..]` or `#![..]`, a macro call named by a fragment or by a repetition, the `tt` fragment, `macro_rules!` inside a `macro_rules!` body, and a fragment other than `$crate` in a use item.
    - Hidden code: a first line that starts with `#!` and not `#![` (rustc skips it as a shebang, after a byte-order mark too), and a `cfg` or `cfg_attr` predicate or a `cfg!` argument that names `clippy` or holds a fragment.
    - Manifests (`lint-tables`): a `[patch]` or `[replace]` table in any manifest, and a build script in any package but `syntax`.
  - Limits of the source rules: self-test violations 5d to 5f, 16, 17a to 17d, and 19 to 21 show that each rule fails on its route. These rules do not make the scan complete. The scan reads source text, so it does not see what a proc-macro crate or a third-party macro expands to. The gate code is in the tree, so a change that rewrites xtask can loosen the gates, and no CODEOWNERS file asks for a review of that change. P1 "Carried from P0d" lists each residual.
  - Suppression count: `allow`, `expect`, `warn`, and the clippy and rustfmt tool attributes (for example `#[rustfmt::skip]`) are suppressions. Each entry in `crates/suppressions.toml` has a `count` (default 1) that must equal the suppressions of that lint in the file. An entry that matches no suppression fails as stale.
  - Forbidden lints: the workspace sets `allow_attributes`, `dbg_macro`, `todo`, and `unimplemented` to `forbid`, not to `deny` as D2 says. rustc then rejects a suppression of them, a macro-built one too (E0453, violation 9b).
- **Goldens**: `cargo xtask goldens --update` and violation 14 move to P1, with the first golden files.
- **Self-test**: the base passes 15 checks, and each of 86 violation checks fails its gate: violations 1 to 13 and 15 to 23 (measured with the uv tool gate binary: exit 0, 101 checks). The base adds `static-gates`, `policy`, a check that the `cargo-gates` and `cargo-policy` triggers match the file names their gates read, a `lint-tables` run with a `syntax` crate (the control for 10b), and a `policy` run with a `Policy-Change` trailer (the control for 22). Violation 8 commits a zero baseline as the base. Each command runs under a 120 s deadline. The new violations:
  - 4b: `health` cannot measure a file.
  - 5c to 5o: a comment inside `#[expect]`, `#[path]`, `include!` (also renamed by a `use` item), an attribute from a macro fragment, `warn`, clippy and rustfmt tool attributes, an `expect` in `xtask/`, and `suppressions.toml` entries that do not match (a count, a stale entry, a zero count, a duplicate).
  - 6b and 6c: no package under `crates/`, and a path dependency outside the workspace.
  - 7b and 7c: a lib target at a custom path, and a `public-api.txt` in a package with no library target.
  - 8b to 8j: a tree baseline above its lowest committed version or deleted, a `--base` that is missing, an option, not a commit, or given twice, a raised baseline on a branch whose base has none, a tree baseline above `health` with no committed version, and `health` at another complexity threshold.
  - 9b: a macro-built `allow` of a lint at `forbid`.
  - 10c to 10g: `unsafe` in `syntax`'s `build.rs` and in a nested `ffi.rs`, `macro_rules!` in `ffi.rs`, a nested crate with no lint table, and `#[path]` under `lint-tables`.
  - 15a to 15h: a stray `clippy.toml`, `.clippy.toml`, `.cargo/config.toml`, and `rustfmt.toml`, an extra root cargo alias, a `deny.exceptions.toml`, a `.deny.toml`, and a nested `.agentless-mcp.json`.
  - 16: a symlink under `crates/`.
  - 17a to 17d: a `.txt` crate root, a crate root outside its package, and a package outside `crates/` and `xtask/`.
  - 18: `static-gates` fails when one of its gates fails.
  - 19a to 19l: a `#` passed to a macro, before a fragment, and joined to a group, a doc comment split by `tt` fragments, a `path` attribute built by a macro, `include!` named by a fragment and by a repetition, `include` named by a fragment in a use item, the identifier `include`, a shebang line, `macro_rules!` inside a `macro_rules!` body, and a `.RS` file.
  - 20a to 20d: `cfg(not(clippy))`, a fragment in a cfg predicate, `cfg!(clippy)`, and a build script in `kernel`.
  - 21a to 21c: `[patch]`, `[replace]`, and a package nested under `crates/io`.
  - 22 and 22b: a policy change with no trailer, and an uncommitted policy change.
  - 23a and 23b: a gate file name that the `cargo-gates` trigger misses, and a trigger that drops a gate file name.
- **Gate binary**: CI clones this repository into `$RUNNER_TEMP`, checks out `GATE_REV`, and runs `uv sync --frozen --no-dev`, so the gate's dependencies come from its `uv.lock`. The clone stays outside the workspace, because `lint --since` would read it there. Until P0c publishes 0.10.1, `GATE_REV` is `9bb51ac`, the P0a merge that added `--fail-on`. P0c moves `GATE_REV` to the 0.10.1 release commit in its own commit. The pre-push hook runs the uv tool install and does not check its version.
- **Erosion with no change**: `cargo xtask erosion` skips `lint` when the tree equals the base and has no untracked files, because `lint` reports an empty change set as `not_checked`.
- **Erosion scope**: `lint --since` covers the whole change set, Python included, and only `health` reads `crates/` (D4 names `crates/` for both).
- **Erosion ratchet**: the reference is the lowest value of each figure across the base's `crates/erosion-baseline.json`, when the base has one, and every version of it committed in base..HEAD. The tree's baseline must not exceed the reference, and `health` must not exceed the tree's baseline or the reference. With no committed version, the tree's baseline must not exceed what `health` measures now. A deleted baseline fails when a committed version exists. `--update` writes the lower of each figure. So no commit can raise the baseline above an earlier committed version. When the base has no baseline, the commit that creates one sets the first reference (P1 "Carried from P0d").
- **Health pins**: `cargo xtask erosion` fails when `health` counts units at a complexity threshold other than 10, stops at its time budget, does not walk every file it finds, or cannot measure a file. It fails before the ratchet runs, so `--update` never writes partial figures.
- **Policy gate**: `cargo xtask policy [--base REV]` judges the policy items: `.agentless-mcp.json`, `deny.toml`, `rustfmt.toml`, `rust-toolchain.toml`, `.config/nextest.toml`, `crates/layers.toml`, each `clippy.toml` and `.clippy.toml` in any case (ignored files too), and the `[workspace.lints]`, `[patch]`, and `[replace]` tables of `Cargo.toml`.
  - The reference for an item is its value at the base (by default, the merge base with `origin/main`). When the base lacks the item, the reference is its value at the commit in base..HEAD that created it.
  - The gate compares parsed values, so a change to comments or layout is no difference.
  - A tree value that differs from the reference fails, unless each commit that changed the item names it in a `Policy-Change: <path>` trailer. The path for a `Cargo.toml` table is `Cargo.toml`. A merge commit that keeps the value of one parent changes nothing.
  - An uncommitted change fails, because no trailer can name it.
  - CI runs the gate before the other gates, and the `cargo-policy` hook runs it at pre-push.
- **Base on a push to main**: the merge base with `origin/main` is then HEAD, so CI passes the push's `before` commit as `--base` to `policy` and `erosion`, unless it is the zero SHA.
- **CI**: a `cargo metadata --locked` step fails on a stale `Cargo.lock` before the first build. install-action runs with `fallback: none`, so it installs each tool from its own manifest, with a sha256 check, or fails. cargo-public-api 0.52.0 builds from source with `cargo install --locked`, because install-action does not ship it. The grammar cache lives under `$RUNNER_TEMP`, outside the workspace that `lint --since` reads. CI never cancels a run on main or on a `v*` tag.
- **Latency (D8)**: the script measured `1dc4d70` and `0037919`, the 0.10.1 candidate. P0c runs it again on the `v0.10.1` tag. Its p95 is the nearest rank, so the overview table cites the script's runs only.
- **Hooks**: D6 named `cargo fmt` and `cargo clippy` only. Each Python check has a Rust counterpart:

| Check | Python | Rust | Runs at |
|---|---|---|---|
| Format | ruff format | `cargo fmt --check` | pre-commit, CI |
| Lint | ruff | `cargo clippy -D warnings`, pedantic | pre-commit, CI |
| Types | mypy | rustc, through clippy | pre-commit, CI |
| Layers | import-linter | `cargo xtask layers` | pre-commit, CI |
| Dependencies | deptry | `cargo machete`, `cargo deny` | pre-commit, CI |
| Suppressions and lint tables | none | `cargo xtask suppressions`, `lint-tables` | pre-commit, CI |
| Spelling | codespell | codespell | pre-commit |
| Tests | pytest | `cargo nextest`, `cargo insta test --check` | pre-push, CI |
| Public API | none | `cargo xtask public-api` | pre-push, CI |
| Erosion | the erosion hook | `cargo xtask erosion` | pre-push, CI |
| Policy files | none | `cargo xtask policy` | pre-push, CI |

### P0d exit gate

- The `rust` CI job is green on both platforms.
- `scripts/gate-selftest.sh` shows the base passing its 15 checks and each of its 86 violation checks failing its gate (violations 1 to 13 and 15 to 23). Violation 14 moves to P1 (P0d as built).
- The grammar pin and the per-grammar revision list are recorded (D1).
- `scripts/hook_latency.py` has measured the baselines, and the design records are in `docs/analysis/` (D8).

## Risks

| Risk | Response |
|---|---|
| No language-pack crate version matches 1.14.3 and links statically | D1 runs before P0b merges. Bump both pins together in P0b with a predicted golden diff. If D1 finishes after 0.10.1, the bump needs 0.10.2, a new frozen base, and a new stored Loc-Bench arm. |
| A change to stored rows (the B1 to B4 schema bump, 16 to 17) lands after 0.10.1 | The bump is in P0b, so it lands before P0c freezes the base. A later one needs 0.10.2, a new frozen base, and a new stored Loc-Bench arm. |
| B3 changes stable ids that an agent holds across calls | Stable ids are per-session handles, not stored contracts. Record the change in CHANGELOG under Changed. |
| Block mode in `.agentless-mcp.json` stops a needed Python defect fix | A complexity finding names its fix: extract a function. Do not lower the mode. |
| The gate binary itself has a defect | It is pinned. A gate defect is fixed in a new Python patch release, and the pin moves in its own commit. |
