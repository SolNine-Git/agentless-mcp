# Rust Port, Phase 1: The Per-Write Path

**Date**: 2026-10-10 | **Status**: not started | **Overview**: [2026-10-10_rust-port-00-overview.md](2026-10-10_rust-port-00-overview.md) | **Depends on**: P0 (0.10.1 released, workspace and gates live) | **Branch**: `feat/rust-port` | **Est. Rust lines**: about 8,000

## Goal

Replace the per-write path with one Rust binary. The binary runs the three pair checks (`complexity_delta`, `contract_drift`, and `clone_delta`) and the `lint` and `capabilities` commands. It also runs the erosion hook in process, reading the Python `tags.db` as decision 9 says. P1 passes only when a measured hook call meets the latency gate in each cache state, and when the tokenizer and the pair checks match the 0.10.1 parity base.

## Scope

- **In**: the crates `pycompat`, `prompts`, `syntax`, `metrics`, `io`, `git`, `store` (the read-only `tags.db` reader), `checks`, `app` (lint pair forms, capabilities), `render` (lint, capabilities, envelope), `cli` (lint, capabilities), `hook` (erosion gate), and `agentless`.
- **Out**:
  - The Rust store's own file, its writer, and `index` (P2). Until P4, the Python MCP server and Python `agentless-mcp index` are the only writers of the cache the hook reads.
  - Symbol facts, the graph, and every read operation (P2).
  - The `--candidates` lint form, which needs SEARCH/REPLACE parsing (P3).
  - The seven patch checks (P3).
  - The MCP server and the search-gate hooks (P4).

## Crates at the end of P1

| Crate | Layer | Depends on (P1 subset) |
|---|---|---|
| `kernel` | L0 | none (from P0) |
| `pycompat` | L0 | none |
| `prompts` | L1 | kernel |
| `syntax` | L1 | pycompat, kernel |
| `io` | L1 | kernel |
| `metrics` | L2 | syntax, pycompat, kernel |
| `git` | L2 | io, kernel |
| `store` | L3 | metrics, io, kernel |
| `checks` | L3 | metrics, syntax, prompts, kernel |
| `app` | L5 | checks, store, git, io, metrics, prompts, kernel |
| `render` | L6 | app, prompts, pycompat, kernel |
| `cli` | L7 | app, render, prompts |
| `hook` | L7 | app, render, prompts, io |
| `agentless` | L8 | cli, hook, app, store, git, io |

Each dependency exists when its user is built. `cargo xtask layers` checks this on every commit.

## Binary name

The `agentless` crate builds a binary named `agentless-mcp`, so commands and hook settings keep their spelling. Until P4 the Python 0.10.1 tool owns `agentless-mcp` on `PATH`. The Rust binary is therefore installed outside `PATH`, and called by absolute path from the hook settings and the latency script.

## Port constraints

The gates refuse these forms in every phase. Each row names the gate that fails and the alternative. A gate's message can name less than the row: the messages of the self-test's hook check (`scripts/hook_triggers.py`) name no alternative, and clippy and rustc print their own text. `static-gates` runs `layers`, `suppressions`, and `lint-tables`. P0 "P0d as built" states each rule in full.

| Rule | Gate | Alternative |
|---|---|---|
| An `allow`, `expect`, `warn`, or `ignore` attribute, or a `clippy::` or `rustfmt::` tool attribute, inner or outer, in `cfg_attr` too | `static-gates`. `ground-truth` reports the lint that a macro-built `allow`, `expect`, or `warn` hides, apart from R-5 to R-7 ("Accepted residuals"). An `ignore` is not a lint: the ignored-test count below catches a macro-built one. | Fix the code. For a suppression that the owner approves, write a literal attribute and add a `crates/suppressions.toml` entry with the file, the lint (`ignore` for `#[ignore]`), the count, and the reason. |
| More or fewer ignored tests in `cargo nextest list` than the `ignore` counts in `crates/suppressions.toml` | `ground-truth` | Run each test. For an `#[ignore]` that the owner approves, write a literal attribute with its ledger entry. |
| A suppression of `allow_attributes`, `dbg_macro`, `todo`, `unimplemented`, `unexpected_cfgs`, or, outside `syntax`, `unsafe_code` | clippy and rustc (E0453): these lints are at `forbid` | None. Fix the code. |
| `unsafe` or `global_asm` outside `crates/syntax/src/ffi.rs`, or `macro_rules!` inside it | `static-gates` | Move the item into `ffi.rs`, and write each unsafe item out in full. |
| The identifier `include`, anywhere | `static-gates` | Rename the item, for example the `include` parameters of `util/fslimits.py`. |
| `#[path]`, `include!`, or a use item that names `include` | `static-gates` | Put the module at its default path. Write the code in a module. |
| The `tt` fragment | `static-gates` | Use typed fragments, for example `($fmt:literal $(, $arg:expr)*)`. |
| A `#` that does not start `#[..]` or `#![..]` | `static-gates` | Write the whole attribute, or pass it as a `#[$name:meta]` fragment. |
| A fragment or a repetition followed by `!`, as in `$name!` (`$x != 1` passes) | `static-gates` | Write the macro's name in the transcriber. |
| `macro_rules!` inside a `macro_rules!` body | `static-gates` | Define each macro at item level. |
| A fragment other than `$crate` in a use item | `static-gates` | Write the path in the use item, or start it with `$crate`. |
| A `cfg`, `cfg_attr`, or `cfg!` predicate that names `clippy` or holds a fragment | `static-gates` | Write a literal predicate that does not name `clippy`. |
| A cfg name or value that rustc and the package features do not declare, or a `check-cfg` list | every build (`unexpected_cfgs = "forbid"`), and `ground-truth` | Use a cfg that rustc knows, or a feature that the package declares. |
| A first line that starts with `#!` and not `#![` | `static-gates` | Delete the line. |
| A file under `crates/` or `xtask/` other than a package manifest, a `.rs` file, a library's `public-api.txt` beside its manifest, the gate data, the `clippy.toml` of `crates/io`, `crates/store`, or `xtask`, or a file in a `DATA_DIRS` directory. The walk skips a git-ignored file that is neither a `.rs` file nor a tool config, for example an editor's swap file. | `static-gates` | Move the file out, or name its directory in `DATA_DIRS` in `xtask/src/sources.rs` in review. P1 needs entries for `crates/pycompat/fixtures/`, `crates/syntax/languages/`, `crates/checks/fixtures/`, the store's `tags.db` fixture, each insta `snapshots/` directory, and the vendored grammar sources if they live under `crates/`. |
| Under `crates/` or `xtask/`, a symlink, a FIFO or another special file, or a `.rs` extension in another case. Anywhere in the repository, a non-ASCII path or a path with an ASCII control character. | `static-gates` | Copy the link's target. Remove the entry. Rename it with `.rs`, ASCII characters, and no control characters. |
| A clippy, rustfmt, rust-toolchain, cargo-deny, `.cargo/`, `.agentless-mcp.json`, `.ignore`, or `.rgignore` file outside the allowed set, tracked, untracked, or ignored | `static-gates` | Put the setting in the root file. Write a test's config at run time, for example a `projectconfig` fixture in a temporary directory. |
| A package that reads a clippy config other than the root `clippy.toml`, or for `crates/io`, `crates/store`, and `xtask` other than its own `clippy.toml` | `static-gates` | Add `crates/io/clippy.toml` and `crates/store/clippy.toml` in the commit that adds the crate. Remove a nearer clippy config. |
| A `[lints]` table other than `workspace = true` (for `syntax`, other than the workspace table without `unsafe_code`, plus the two unsafe lints at deny) | `static-gates` | Write `[lints] workspace = true`. |
| An `edition` or `rust-version` other than the workspace's, in a package or a target table | `static-gates` | Write `edition.workspace = true` and `rust-version.workspace = true`, and no `edition` in a target table. |
| `test = false`, `required-features`, `harness = false`, or `autotests = false` that leaves a file in `tests/` without a target | `static-gates` | Remove the key. Write `#[test]` functions. Name the test file in a `[[test]]` table. |
| `[profile]`, `[package.metadata.cargo-machete]`, `[patch]`, or `[replace]` in a package manifest | `static-gates` | Set a profile in the root manifest with a trailer. Remove the unused dependency. Depend on a published version, or make the crate a workspace member. |
| A build script in a package other than `syntax` | `static-gates` | Move the logic into the crate's code. P1 adds `cargo xtask generate` in work item 2. After that, generated Rust that the commit holds is a second alternative. |
| A package outside `crates/` and `xtask/`, or a target source that is not a `.rs` file inside its package | `static-gates` | Move the package or the source. |
| rustc reads a file other than a scanned `.rs` file, a `Cargo.toml`, or a clippy config, through `include_str!`, `include_bytes!`, `include!`, or `#[path]`, also when a macro builds it. The gate compares each path after it resolves symlinks. A dep-info file that does not parse, or an input that does not exist, breaks the gate. | `ground-truth` (dep-info) | Read the data at run time. A test reads its fixtures at run time. P1 adds `cargo xtask generate` in work item 2. After that, generated Rust that the commit holds is a second alternative. No list of approved compile-time reads exists: `dep_info.rs` allows only the scanned sources, the manifests, and the clippy configs. To approve a compile-time read, add a reviewed list to `xtask/src/ground_truth/dep_info.rs` with a self-test row. The `include_str!` plans in P2 (guide, HTML template) and P4 item 6 meet this rule. |
| A `[workspace.lints]` table other than `rust` and `clippy` | `ground-truth` breaks | Teach `xtask/src/ground_truth/lints.rs` the tool in review. |
| A change to a policy file, or to the root `Cargo.toml` outside `[workspace.dependencies]` and the `[workspace.package]` version (for example `members`, the `rust-version` bump, P4's `[profile.release]`) | `policy` | Put `Policy-Change: <path>` in each commit that makes the change, with `Cargo.toml` as the path for the root manifest. The owner reviews it. When the base lacks the file and two or more commits in the range create it, the first creation commit needs the trailer too. |
| A third-party package that `crates/dependencies.toml` does not list. `direct` lists each dependency of a workspace package that is not a path dependency, of any kind and target. `build-time` lists each package in the resolved graph with a proc-macro or build-script target. A listed name that the graph no longer holds fails too. | `static-gates` (`layers`) for `direct`, `ground-truth` for `build-time`, and `policy` for a change to the list | After review, add or remove the name in a commit with a `Policy-Change: crates/dependencies.toml` trailer. |
| A merge that brings a policy value from outside the range | `policy` | Redo the merge so that it keeps the value from inside the range. Then change the value in a commit with a trailer. |
| A tracked file with an assume-unchanged or skip-worktree mark, a sparse checkout too | `erosion` | Clear each mark with `git update-index --no-assume-unchanged --no-skip-worktree <path>`. |
| A new cargo hook in `.pre-commit-config.yaml` | the self-test's hook check | Add the hook's stages and inputs to `scripts/hook_triggers.py`. |

## Do in P1

- When `syntax` adds the tree-sitter crate, set the workspace `rust-version` to that crate's minimum, in a commit with a `Policy-Change: Cargo.toml` trailer.
- `crates/io/clippy.toml` (the root file without the `std::fs` entries) and `crates/store/clippy.toml` (without `rusqlite::Connection::open`) arrive in the commit that adds their crate. The tool-config check fails an exempt package that does not read its own file.
- When `syntax` arrives, add `clippy::undocumented_unsafe_blocks`, which its lint table sets at deny, to the ground-truth force-warn set. The set comes from `[workspace.lints]` and the edition 2024 lint table, which already warns on `unsafe_op_in_unsafe_fn`.
- `cargo xtask goldens --update` and self-test violation 14 (a changed golden file) arrive with the first golden files.
- Generated Rust is committed source. The gates refuse `include!`, so no crate reads code from `OUT_DIR`. `cargo xtask generate` writes the Rust source into `crates/<crate>/src/generated/`, and the commit holds the file. An xtask test fails when a committed file differs from a fresh generation. The static gates and clippy then read the generated code as they read any other source. Only `syntax` keeps a `build.rs`, which compiles the grammar C sources with `cc` and writes no Rust (work items 2 and 3).
- Do the work that each item of "Accepted residuals" marks as "P1 work".

## Accepted residuals

Each item is a route that the gates leave open after P0d, or a review finding that waits for P1 work. Reviews and the xtask docs cite these ids, so the ids are stable: a new item takes the next free number, and no id is used twice.

### Macros and the compiler checks

- **R-1**: The token scan reads source text, so it does not see what a macro expands to: an attribute that a proc-macro crate builds from strings, or an attribute, a cfg, or `unsafe` code that a third-party macro emits. `ground-truth` asks the compiler instead. Its force-warn ledger reports the lint that a macro-built suppression hides, apart from R-5 to R-7. Its ignored-test count catches a macro-built `#[ignore]` (self-test 25g). It does not see code that a macro-built cfg hides from clippy (R-2). clippy rejects a suppression of a clippy lint at `forbid`, a macro-built one too (E0453), and rustc rejects one of a rustc lint at `forbid`, such as `unsafe_code`. A third-party macro comes only from a package that `crates/dependencies.toml` lists. A new direct dependency fails `layers`, and a new build-time package fails `ground-truth`, until a commit with a `Policy-Change: crates/dependencies.toml` trailer lists it (self-test 6d, 25f, and 22v). The workspace has no proc-macro crate.
- **R-2**: A listed crate's macro can apply `cfg(not(clippy))` to the caller's code. That code ships, and no clippy run sees it. The token rule refuses only a literal predicate that names `clippy`. Measured with a declarative macro from a listed crate that hides a `dbg!` and an `unwrap` this way: `static-gates`, `ground-truth`, `cargo clippy -D warnings`, and `cargo nextest run` exit 0. Only the review of the trailer that lists the crate stands against this route.
- **R-3**: A version change of a listed crate needs no trailer, because package manifests, `[workspace.dependencies]`, and `Cargo.lock` are not policy. A new version can add a macro of the R-2 kind. A new build-time package that it brings fails `ground-truth` (R-4).
- **R-4**: `ground-truth` compares the `build-time` list with the resolved graph after its own clippy build. So the build script or proc macro of an unlisted package has run once when the gate fails it.
- **R-5**: The force-warn ledger matches a diagnostic by target and lint, without counts. An entry for a group or for `warnings` covers each member lint in every file of the target (measured: a `clippy::pedantic` entry for one file covered `clippy::must_use_candidate` in another file of the same target). So a suppression that a macro or a derive builds passes when its target already lists that lint, a group that holds it, or `warnings`.
- **R-6**: Under `--force-warn`, the test build of a bin reports its `main` and each function that only `main` reaches as dead code, so the ledger check leaves out `dead_code` in a bin's test build. A suppression of `dead_code` in a bin's test-only code therefore passes.
- **R-7**: clippy and rustc skip most lints in external macro output. So code that a dependency's macro or a workspace proc macro emits is not linted, and a suppression there is not seen. A file that a proc macro reads during expansion is not in dep-info, so no gate reads it (measured: `static-gates`, `ground-truth`, and clippy exit 0).
- **R-8**: `unexpected_cfgs` accepts the cfg names and values that rustc knows: `clippy`, `docsrs` (cargo's default check-cfg), `miri`, `doc`, `doctest`, and the target and panic values. Each clippy run sets `clippy`, and no shipped build sets it, so code under `not(clippy)` ships with no clippy run. The cfg rule refuses a literal predicate that names `clippy`, and R-2 covers a macro-built one. Code under one of the other names that the two CI builds do not set is outside every clippy run.
- **R-9**: A cfg that no CI build sets, such as `cfg(false)`, `cfg(any())`, or `cfg(miri)`, removes a test before nextest lists it, as a deletion does. No gate fails, and nextest reports no skip (measured). The ignored-test count does not see such a test. A literal `#[ignore]` with its ledger entry on a removed test can balance the count of a macro-built `#[ignore]`, and review sees both lines.
- **R-10**: A test module that two test binaries compile appears twice in the nextest list. So one `#[ignore]` in it counts twice in the ignored-test count, and the ledger can count it only once (derived, not run).
- **R-11**: The build-script rule lists only `syntax`. That crate's `build.rs` can still declare a cfg with `cargo::rustc-check-cfg` and set it, and that cfg can hide code from clippy.
- **R-12**: clippy runs the dev profile with the default features. So it does not lint code under `not(debug_assertions)`, behind a feature that is off, or under a `[profile.test]` setting that differs from the dev profile. The review of `7a08268` reproduced the first and the last. A root `[profile.*]` table is policy, so a profile change needs a trailer, and only review of that trailer holds it. Run clippy with `--release` or `--all-features` when those builds exist.
- **R-13**: CI lints and tests Linux x86_64 and macOS arm64 in the dev profile. Code under a cfg that only another shipped target or the release profile turns on, such as `all(target_os = "macos", target_arch = "x86_64")` or `panic = "abort"`, passes the static gates and clippy (measured on Linux x86_64). P4's release job lints and tests each wheel target and the release profile (P4 item 4). Until then, no gate checks that code.
- **R-14**: `cfg_select!` is unstable on 1.94.0 (E0658), and the cfg rule does not read it. Add it to the cfg rule when the toolchain moves to a release that accepts it.
- **R-15**: A frontmatter block (`---` at the top of a file) passes the gates. rustc 1.94.0 rejects it (E0658). Check it again when the toolchain moves.
- **R-16**: rustdoc compiles doc examples, but the token walker reads their text as a literal, and nextest does not run doctests.
- **R-17**: No lint refuses `Command::output()` outside `xtask/src/process.rs`, the one subprocess helper.
- **R-18**: No gate checks the environment or config above the repository root (`CLIPPY_CONF_DIR`, `RUSTFLAGS`, `RUSTC_WRAPPER`, `CARGO_HOME` config). `ground-truth` runs `cargo clippy` with the caller's environment, so such a setting can change what the compiler judges at pre-push. CI's `rust` job sets none of them (read from the step environments of run 38133337220), and nothing checks that. P1 work: clear these variables in `cargo_clippy::clippy` with `env_remove`: `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, each `CARGO_TARGET_<triple>_RUSTFLAGS`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`, `CLIPPY_CONF_DIR`, `CARGO_BUILD_TARGET`, and each `CARGO_PROFILE_DEV_*`. Do not refuse them, because a refusal stops an sccache user at pre-push. A config file above the root stays open.
- **R-19**: The tool-config check lists ignored files through name pathspecs that fold ASCII case only. So a git-ignored tool config outside `crates/` and `xtask/` whose name holds a non-ASCII letter, for example `ruſtfmt.toml` named in `.git/info/exclude`, is not listed. APFS folds such a name onto the ASCII one: in CI run 38133337220, `cp` on the macOS job reported `ruſtfmt.toml and rustfmt.toml are identical`. CI is unaffected, because its checkout holds no ignored file.

### Policy, erosion, and git

- **R-20**: No gate checks the insta config. `.config/nextest.toml`, where a default filter can skip tests, is a policy item, so it changes only with a `Policy-Change` trailer.
- **R-21**: The root `Cargo.toml` is a policy item except `[workspace.dependencies]` and the `[workspace.package]` version. So a `[workspace.metadata.cargo-machete]` list, a change to the workspace `edition` or `rust-version`, a `[profile.*]` table, or a `members` or `exclude` change needs a `Policy-Change: Cargo.toml` trailer, and only the owner's review of that trailer judges it. In a package manifest, the static gates refuse a `[profile]` table, the cargo-machete metadata, and an `edition` or `rust-version` off the workspace's.
- **R-22**: A new `[workspace.package]` table that holds only `version` needs a trailer, because the policy view keeps the empty table.
- **R-23**: A policy file that the base lacks is accepted as its creation commit wrote it, when exactly one commit in base..HEAD creates it (a commit that holds the file while each of its parents lacks it). That commit is the reference and needs no trailer. When two or more commits create the file, none is exempt (self-test 22p). So a file that a branch deletes and creates again needs a trailer on its first creation commit too, and on a pushed branch that means a rewrite of history.
- **R-24**: When the base has no `crates/erosion-baseline.json`, the commit that creates it sets the first ratchet reference. The gate bounds the baseline by `health` only while no committed version exists (derived from `xtask/src/erosion.rs`, not run).
- **R-25**: On a pull request, CI's HEAD is GitHub's merge commit, which carries no trailer. If main and the pull request both change one policy file, and git merges them cleanly, the policy gate fails on that merge (derived from `xtask/src/policy.rs`, not run).
- **R-26**: The policy gate reads each trailer from the raw commit (`git cat-file commit`), and its git calls ignore replace refs and `.git/info/grafts`. So a trailer alias, `core.commentChar`, a replace ref, or a graft does not change its verdict (self-test 22q to 22u). The pinned Python lint gate still builds its change set through a working-tree diff. So a local clean filter (a `filter.<name>.clean` command and an attributes line) can hide a committed edit from `lint --since` at pre-push (measured). When the filter hides every changed file, `lint` reports `not_checked`, and `cargo xtask erosion` fails closed (self-test 8o). CI has no such config. The fix belongs in the Python gate's change detection (stored trees, as xtask's own change test reads them), or in an xtask refusal of a tracked file with a filter attribute. A refusal of every `filter.*` setting would stop git-lfs users.
- **R-27**: The gate code is in the tree. A change that rewrites xtask, `ci.yml`, `.pre-commit-config.yaml`, or the self-test (`scripts/gate-selftest.sh`, `scripts/deadline.pl`, `scripts/hook_triggers.py`) can loosen the gates that judge it. For example, a hook entry `cargo xtask static-gates || true` turns a local gate off, and the self-test's hook check does not read hook entries. Until P0c sets the branch protection and adds a CODEOWNERS file (P0 "P0c"), the Rust jobs are not required status checks, `main` allows squash and rebase merges, and nothing asks for a review of those paths.

### Processes

- **R-28**: xtask builds on Unix only, because it puts each child process in its own process group through `std::os::unix::process::CommandExt`. CI runs Linux and macOS (overview decision 5). At a deadline, or when a descendant still holds a pipe 2 s after the child exits, xtask kills the child's group. A process that leaves its process group (setpgid or setsid) after its parent exits survives (measured with a job-control group). After the leader is reaped, an empty group's id can be reused, so the kill can reach another group (derived from the code, not run). P1 work: the comment in `held` (`xtask/src/process.rs`) names setsid only. Reword it to name both calls.
- **R-29**: `process::run` leaves the child and its group alive when `try_wait` or a reader thread fails after the spawn: the error returns as broken with no kill (found by reading, not triggered). P1 work: arm a guard after the spawn that kills the group on every early return, and disarm it on success. Its comment states the group-id reuse of R-28.
- **R-30**: The self-test's deadline helper (`scripts/deadline.pl`) and `scripts/hook_latency.py` kill each process below the command that `ps` finds, and their process groups. A process that leaves its process group (setpgid or setsid) after its parent exits escapes both. It can hold a self-test row's output open past the deadline: probes measured 6.0 s with setsid and 30.0 s with setpgid for a 2 s deadline. When perl itself dies, for example from a SIGKILL, the session leader that `deadline.pl` forks sees the change of its parent within 20 ms and kills the command's processes (a self-test base check).
- **R-31**: `scripts/hook_latency.py` has no such leader. If python itself dies from a SIGKILL, its `finally` does not run, and the command's session survives (derived from the code, not run). It is a development-time tool.

### CI

- **R-32**: The quality jobs restore the grammar cache through a `restore-keys` prefix, so they can restore a cache that another key saved. The docstring of `src/agentless_mcp/core/grammars.py:24-26` says that the pack checks a downloaded bundle against the sha-256 in its name, and the pack's native module defines a checksum error. That digest comes from the same source as the bundle, and no check of a restored bundle is known. The `rust` job's gate warmup also downloads prebuilt grammar libraries for the gate binary. setup-uv restores uv's pruned package cache (17,531 bytes on the ubuntu job of run 38133337220), which serves the gate binary and the self-test's hook check. Neither serves the cargo tools.
- **R-33**: Every CI run builds cargo-public-api 0.52.0 from source with `cargo install --locked --force`, because install-action does not ship it. rust-cache keeps no `~/.cargo/bin`, so no cached binary of it reaches the job. Run 38133337220 built it in 1 min 04 s on ubuntu and 1 min 00 s on macOS. That build runs the build scripts and proc macros of its dependency tree before the gates run, and the workflow pins no digest of the crate or of the binary. So CI trusts cargo-public-api and its dependencies at the version pin. install-action still downloads the release binaries that each tool's project publishes (cargo-nextest, cargo-deny, cargo-machete, cargo-insta) and checks them against sha256. This workflow does not build them.
- **R-34**: CI's `cargo metadata --locked` step proves `Cargo.lock` before the first build, but the `cargo +nightly public-api` run inside xtask takes no `--locked` flag.
- **R-35**: A push run on `main` that fails, or that someone cancels by hand, leaves its range without a green judgment. The tag run of that commit judges an empty range, because the merge base is HEAD.

### Self-test

- **R-36**: The self-test stops each command at 120 s. The figure is derived: about ten times the slowest cold command measured with one build job (11.4 s). If a command in a cold CI run takes longer, raise `step_deadline` from the row times in that run's log.
- **R-37**: Self-test violations 5b, 7, 9, 12, and 12b still match short texts (`dead_code`, `public-api.txt`, `disallowed`, `itoa`) that another failure can also print.
- **R-38**: Two self-test rows prove less than their names say, and one break condition has no row. Row 22o passes when either of its two defenses is reverted (`-c log.follow=false`, or the removal of inherited `GIT_` variables), so it proves the pair, and a unit test covers the removal. Row 25c passes with `unexpected_cfgs = "warn"`, because the force-warn run prints the same text, and row 25d owns the level. No row covers a condition that breaks `ground-truth`, for example a `[workspace.lints]` table other than `rust` and `clippy` ("Port constraints"). P1 work: name each row after the defense that it proves, and add a row for an unknown lints table.
- **R-39**: The self-test labels its rows with codes such as 5g, 15q, and 22o, and nothing maps a code to its gate, but the plans and the reviews cite the codes. P1 work: add an index from row code to gate at the top of `scripts/gate-selftest.sh`, or print the gate name in each `ok` line.
- **R-40**: cargo reads a `.cargo/config.toml` in any parent directory of the self-test's scratch directory, and the self-test does not isolate that. A config file above `TMPDIR` made the self-test report rows 11a and 13 as failed on a tree with no defect (measured). A local run is exposed only when such a file exists.

### P1 decisions and upkeep

- **R-41**: clap's derive does not compile under the workspace `forbid` levels: `#[derive(Parser)]` emits an `allow` of `clippy::restriction`, and rustc refuses it (E0453). Measured with clap 4.6.7: clippy exit 101, `ground-truth` exit 1. tokio's `#[tokio::main]` and `#[tokio::test]` pass both. Work item 9's `op!` macro emits the clap arguments. P1 work: decide whether `op!` emits clap builder calls with no derive, or whether a `forbid` level becomes `deny`. Record the decision in "Port constraints".
- **R-42**: The 60 s deadlines of the ground-truth clippy runs and of its `cargo nextest list` come from the P0d tree. A cold run builds everything, and a check build still runs build scripts. Work item 3 compiles the C sources of 22 grammars in the `build.rs` of `syntax`. P1 work: measure a cold `ground-truth` run when `syntax` lands, and set the deadlines from that figure.
- **R-43**: Five xtask files each define `const MANIFEST: &str = "Cargo.toml"`. P1 work: keep one `pub(crate) const MANIFEST` in `xtask/src/workspace.rs`, and import it in the other files.

## Work items, in build order

### 1. `pycompat`

- **Functions**: `float_repr`, `round_half_even`, `sum_builtin` (CPython 3.12+ built-in `sum()` of floats, which is compensated), `splitlines`, `split_whitespace`, `strip`, `PyJson` (a serde_json `Formatter`: `indent=2`, `ensure_ascii` with surrogate pairs, `": "` separators), `max_first`, and `python_sloc`. A plain `+=` loop in Python is a plain Rust loop, not `sum_builtin`. Each call site in the port names which of the two it reproduces.
- **`python_sloc(src) -> Result<BTreeSet<u32>, TokenizeError>`**: the CPython 3.13 tokenize rule, on lines from `splitlines(keepends=True)`.
  - A line counts when a token other than COMMENT, NL, NEWLINE, INDENT, DEDENT, or ENDMARKER starts on it.
  - Covers string prefixes, triple quotes, f-strings with nesting (PEP 701), line continuation, brackets, and indentation errors.
  - Rejection matches `tokenize.TokenError` and `SyntaxError`, including `IndentationError`, as a `TokenizeError` kind.
- **Python sources**: `src/agentless_mcp/core/complexity.py:298-323` (the SLOC rule); `src/agentless_mcp/application/envelope.py:457-459` and `render.py:77-86` (JSON form and negative zero).
- **Fixture generator** (development time only): `scripts/pycompat_fixtures.py`.
  - It runs under a pinned CPython 3.13 through `uvx`. CI also tests Python 3.10, where `sum()` is not compensated; the fixtures and goldens come from 3.13 only.
  - It writes `crates/pycompat/fixtures/*.json`: inputs and expected outputs for every function.
  - The tokenizer corpus is every `.py` file in the fixtures, a frozen copy of mcp-local `src/` at f17af96, and the Python files of the Loc-Bench checkouts.
  - Tests read only the committed JSON. No test runs Python.
- **Fuzz job** (opt-in, nightly): generate Python source with proptest. Compare `python_sloc` with `python3 -m tokenize`, and store each mismatch as a new fixture.
- **Done when**: every fixture passes, and the corpus shows zero differences in line sets and rejection kinds.

### 2. `prompts`

- `cargo xtask generate` reads `src/agentless_mcp/prompts/*.json` (the files move into the crate when the Python tree is deleted). It writes `crates/prompts/src/generated/` with one typed function per message, and each placeholder becomes a typed argument. The commit holds the generated file ("Do in P1"). This item adds the `cargo xtask generate` task.
- A missing or extra placeholder fails the generator. `deny_unknown_fields` applies, because the project owns these files.
- `contract_rules()` returns the parsed `contract_rules.json`, validated eagerly.
- **Tests**: render every message once; parse every contract rule.

### 3. `syntax`

- **`Language`**: an enum of the 22 languages, with `from_path` and `from_name`.
- **`LangSpec`**: one TOML file per language in `crates/syntax/languages/`. `cargo xtask generate` turns them into static tables in `crates/syntax/src/generated/`, under the same test as `prompts`. The P1 fields are those that metrics and checks read:
  - extensions and tier;
  - comment, identifier, literal, unit, bound, scope, and descend kinds;
  - decision points, with `Cond` predicates and field and token operators;
  - clone candidate kinds and clone literal classes;
  - the docstring rule and the SLOC rule.
- **Sources for the tables**:
  - `src/agentless_mcp/core/complexity.py:501-775` (RULES)
  - `src/agentless_mcp/core/clones.py:204-298` (CANDIDATES)
  - `src/agentless_mcp/core/normalize.py:265-393` (clone identifiers and literals)
  - `src/agentless_mcp/core/extractor.py:109-125` (comment and indent-block kinds)
  - `src/agentless_mcp/core/grammars.py:78-115` (tiers)
- **Code, not data**:
  - `Cond`, the six predicates at `complexity.py:472-498`;
  - `NameRule`, the name and binding rules at `complexity.py:381-470`. The Rust impl owner uses the one rule that P0b B3 settled.
- **Grammars**: the C sources vendored from `parser-sources-1.14.3.tar.zst`, with `ctype_shim.h` and utf8proc, compiled by `cc` in `build.rs` and checked against a sha256 manifest (overview, Grammar sources). The build script compiles C only and writes no Rust. `tree-sitter` is pinned to `=0.26.9`. `grammar(Language)` performs no IO.
- **`parse(lang, src) -> Result<Parsed, Unparsable>`**. `Parsed` owns:
  - the `Tree`;
  - `Flat`, built in one cursor walk: kind ids, a named bit set, spans, the subtree end index per node, and a field index for name lookups;
  - the source as `Arc<str>`.
- **Queries**: compiled once per language and kind, and validated at start-up.
- **Unsafe**: grammar FFI is the only `unsafe` in the workspace. It lives in `crates/syntax/src/ffi.rs`, under the lint table that P0 D2 sets for `syntax`.
- **Tests**:
  - Every kind name in every `LangSpec` exists in its grammar (`Language::id_for_node_kind`).
  - Every query compiles.
  - On every fixture file, the `Flat` subtree slices equal a cursor walk (proptest over random node indices).
  - Record the `Flat` memory per node on the largest fixture.

### 4. `metrics`

- **`measure(&Parsed) -> Result<FileMetrics, Unmeasurable>`**: one traversal of `Flat` gives:
  - units: qualname, span, body key, cc, and sloc;
  - blocks: span, exact key, Type-2 key, token count, and whether the block is a function;
  - the SLOC line set.
- Nested keys hash their own slices. Python SLOC uses `pycompat::python_sloc`, then removes docstring lines with the ownership test. The byte-column slicing quirk stays on purpose (`complexity.py:356-379`).
- Also `erosion(units, threshold)`, `clone_groups(files)`, and `clone_sloc_lines(...)`.
- **Python sources**:
  - `complexity.py` (`_measured` 132-163, `_walk_units` 185-206, `_decisions` 256-266, `_weight` 269-281, `_leaf_sloc_lines` 284-295, docstrings 330-379)
  - `normalize.py` (`node_stream` 258-262, `_tokens` 555-591, `_directive_text` 594-607, `clone_key` 405-439)
  - `clones.py` (`file_blocks`, `_blocks` 150-165, `clone_groups`, `clone_sloc_lines`)
- **Parity tests**:
  - The 6 complexity goldens (`tests/characterization/goldens/complexity/`): units both ways, cc and sloc exact, erosion within 1e-9, clone groups and lines exact, and `tokenize_rejects` as a typed error.
  - The scb-check snapshot (f17af96 mcp-local `src/`): 1,770 units, 200 groups, 545 instances, and 2,513 clone lines.
- **Property tests**:
  - A comment-only edit keeps the body key.
  - Renaming identifiers keeps the Type-2 key.
  - Reordering units does not change the sorted set of (qualname, cc).

### 5. `io`

- **`Runner`**: one bounded process runner (process-wrap, wait-timeout). `Spec` carries the program, args, cwd, environment allow-list, timeout, stdout cap, and process-group flag. `Outcome` is `Exited`, `Truncated`, `TimedOut`, or `NotRun`.
- **`Fs`**: `walk` (bounded, with the (dev, ino) set), `read` (`ReadOutcome`: text, too large, not UTF-8, or unreadable; the cap is 1,000,000 bytes, as `src/agentless_mcp/util/fslimits.py:35`), `contained` (refuses a path that resolves outside the root), and `append_line` (the hook log). `io` is the only crate that writes files (P0 D2).
- **Config**:
  - `EnvConfig::from_env(get)`: read once at start-up; a bad value fails start-up.
  - `parse_project(bytes)`: serde. An unknown key is dropped with a warning, as `KNOWN_KEYS` does today. The open uses the P0a A2 rules (regular file, contained, `O_NONBLOCK`).
- **Lock and deadline**: `try_lock` (fs4); `Deadline` (Instant, credit, cancel flag), passed explicitly; `walk_budget(remaining)`, the lesser of 10 s and half the remaining allowance (`src/agentless_mcp/util/deadline.py:26`, 93-97).
- **Python sources**:
  - `src/agentless_mcp/core/gitinfo.py:356-470` and `src/agentless_mcp/core/sandbox.py:374-633` (the two runners this replaces);
  - `src/agentless_mcp/util/fslimits.py`, `src/agentless_mcp/core/treewalk.py`, `src/agentless_mcp/core/projectconfig.py`;
  - `src/agentless_mcp/util/cachedir.py:32-55`, `src/agentless_mcp/util/deadline.py`.
- **Tests**:
  - One contract suite runs against the real `Fs` and `Runner` and against their fakes.
  - Timeout and kill behavior use a child that sleeps, with no assertion on wall time beyond the timeout.

### 6. `git`

- **`GitRead`**:
  - `probe(root) -> Result<RepoState, ProbeError>`, with `RepoState::NoGit` as an answer. Python uses four processes and keeps short object ids (`gitinfo.py:284-318`). The Rust probe uses two: `rev-parse --show-toplevel HEAD HEAD^{tree}` and `status --porcelain`. It keeps full ids and shortens them only for display. This is new behavior; the receipt line must stay byte-equal, and the parity harness checks it.
  - `list` (`ls-files --cached --others --exclude-standard -z`).
  - `blob` (`cat-file blob`, bounded).
  - `changes(rev)` (`diff --raw -z -M --relative rev --` plus untracked files).
  - `verify(rev)` (`rev-parse --verify --end-of-options rev^{commit}`).
- **Hardening, kept exactly**: the argv prefix from `src/agentless_mcp/core/gitinfo.py:76-90`; `LC_ALL=C`; every `GIT_*` variable stripped except `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_NOSYSTEM`; the `./` pathspec prefix.
- **Pure parsers**: status count, NUL listing, raw diff records (modes 160000 and 120000), and the binary test (a NUL byte in the first 8,000 bytes).
- **Python sources**: `gitinfo.py:284-318` (snapshot), `src/agentless_mcp/core/gitchanges.py`, and `src/agentless_mcp/core/githistory.py:75-94`.
- **Tests**: parsers on captured output; one integration test per query on a repository the test builds in a temporary directory.

### 7. `store`: the `tags.db` reader (decision 9)

- **`TagsDb::open(root) -> Option<TagsDb>`**: opens the Python cache at the path `src/agentless_mcp/util/cachedir.py` computes, read-only (`mode=ro`, `query_only`). It returns `None` for a missing file, a `meta` schema other than 17 (`src/agentless_mcp/core/cache.py:185`), or a read error, and the caller reports the `no_tag_cache` gap. SQLite WAL readers do not wait for the writer.
- **Reads**: the `files` rows (path, sha256) and the `blocks` rows of each file (`cache.py:2117-2220`). `blocks_for(path, text)` returns the stored blocks when the row's sha256 equals the text's. Otherwise the caller measures the text in memory and writes nothing, as `cache.py:597-606` does.
- **No writer in P1.** The P2 store adds the Rust file, the writer, plan, and eviction.
- **Tests**:
  - A fixture `tags.db`, written by 0.10.1 `index` over a fixture repository and committed: its blocks equal a re-measure of the same files.
  - Schema 15 and 17, a missing file, and a corrupt file each give `None`.
  - A row whose sha256 differs from the text is measured in memory.

### 8. `checks`

- **Types**: `Check` trait, `Needs`, `Inputs` (built lazily for the selected checks), `Finding` and `Detail` (serde), and `Finding::not_checked`. `run(selected, inputs)` applies one error guard and one sort.
- **`cascade<T: Keyed>(old, new, stages)`**: the one matcher. A `Stage` has a scope (per file or global), a key, a mode (first-in-first-out, unique-then-rank, or grouped), and a label. A grouped stage pairs inside each group of its key by a second key first, in order, then pairs the rest in order: the body pass pairs same-qualname units first inside each body-key group (`src/agentless_mcp/core/erosion.py:509-537`). It replaces `erosion.py:484-581` and `837-880`, with `VecDeque` in place of `list.pop(0)`.
- **Path scope**: one function per check, which implements decision 8 as P0a A1 settled it.
- **`complexity_delta`**: the entry at `erosion.py:348-365`, and the body, name, and collision passes at 484-562 (the D2 spec in `docs/plans/2026-10-09_erosion-checks.md`).
- **`contract_drift`**: rules from `prompts::contract_rules()`, run as queries on the `Tree`; hits keyed by (rule, unit, capture), compared as multisets across one run's files; `not_checked` details `no_boundary_globs` and `no_rules_for_language`.
  - **Glob rules** (`src/agentless_mcp/core/contracts.py:40-45`, 156-198), not a glob crate: a glob and a path split on `/`; `*` matches inside one segment; `?` and `[` are literal; a final `**` matches one or more non-empty segments, and an inner `**` matches zero or more; the match is one pass with no backtracking. Port the `TestBoundaryGlobs` cases (`tests/unit/test_contracts.py:631-680`) as a table.
- **`clone_delta`**:
  - Line pairing uses `similar` with `Algorithm::Histogram`, pinned to one exact version (decision 6).
  - Then exact, Type-2, and cross-file stages.
  - Nesting fold; the 40-token floor; at most 3 evidence sites and 20 detail sites.
  - **The index, as at `1dc4d70`**: built lazily, on the first `clone_delta` call with a changed file in scope (`src/agentless_mcp/application/lint_service.py:167`, 345-349). It indexes files in sorted order until `walk_budget` runs out, and each file after that gets the `clone_index_stopped` gap (`erosion.clone_index`, `erosion.py:377-400`). The hook form reads blocks through `TagsDb` only, and without it reports one `no_tag_cache` row per pair (`lint_service.py:351-355`; `erosion.py:410-411`, 954-958). `--since` uses `TagsDb` when it exists and measures in memory otherwise.
- **Message text**: from `prompts`. The finding sort orders match `erosion.py`.
- **Tests**:
  - A table of (old text, new text) to findings per check, for all 12 tier-1 languages.
  - Property tests: a moved unit, a renamed unit, and reordered same-name units give no finding.
  - One test pins the move-plus-copy case from the decision 6 research.
  - The clone index with no cache, a fresh cache, a stale row, and a budget that runs out after one file (injected clock).

### 9. `app` (P1 operations)

- **Types**: `Services` (the port bundle) and `Session` (`RepoState`, `FactSource`, `Deadline`).
- **`op!` macro**: one declaration per operation. Each parameter carries a type, default, bounds (`Limit`), description key, CLI spelling, MCP spelling, and omission rule. The macro emits the serde request struct, the clap arguments, the JSON schema (schemars), and an `OpSpec` entry in `OPS`.
- **Operations**:
  - `lint` in two pair forms: `--path REL --after FILE` (the hook form) and `--since REV`. Flags `--only` and `--fail-on`.
  - `capabilities`: version, `lint_checks` with each input kind, effective config with warnings, and the language capability report.
- **Python sources**:
  - `src/agentless_mcp/application/lint_service.py` (registry 138-188, selection and the two index sources 329-356, pair forms 707-921, `_run_pairs` 959);
  - `src/agentless_mcp/application/capability_service.py`.
- **Tests**:
  - Every operation runs against fake ports (fake `Fs`, `GitRead`, `TagsDb`, and `FactSource`), with insta snapshots of the typed result.
  - A generated cross-door test sends every parameter through the CLI spelling at 0, -1, the cap + 1, and absent.

### 10. `render` (P1 views)

- **Envelope and receipt**: `wrap` and `wrap_json`, with the head, dirty, and cache line formats, the 16,000-token ceiling, the 1/8 block share, the 64-token marker, and at most 8 config warnings (`src/agentless_mcp/application/envelope.py:72-105`, 342-421).
- **Lint text and JSON**: `LintFinding`, `LintCandidate`, and `LintReportView` (`src/agentless_mcp/application/render.py:1434-1580`) and `render_lint` (1581), with `detail` and `observed`.
- **Capabilities** text and JSON.
- **Tests**: insta snapshots over fixed values; the receipt line formats.

### 11. `cli` (P1 commands)

- **`command()`**: built from `OPS`.
- **Exit codes**: 0 ok, 1 operation failed, 2 usage, 3 `--fail-on` matched. Notes go to stderr, with the `agentless-mcp: ` prefix.
- **Python sources**: `src/agentless_mcp/adapters/cli/main.py` (lint 604-659, 1341-1392), and `src/agentless_mcp/adapters/cli/formatting.py:13-23`, 77-97.
- **Tests**: trycmd snapshots of `--help`, usage errors, and exit codes.

### 12. `hook` (erosion gate)

- **Command**: `agentless-mcp hook erosion`. It reads the PreToolUse payload from stdin, capped at 4 MiB. Required fields are validated, and extra fields are ignored (Codex V35).
- **`proposed_text`**: the Write rule (content verbatim) and the Edit rule. The Edit rule:
  1. counts CRLF and bare LF in the old text;
  2. normalizes to LF;
  3. replaces `old_string` once, or every occurrence with `replace_all`;
  4. converts to CRLF when CRLF was the majority (a tie gives LF);
  5. skips the Edits that Claude Code itself fails.
- **Environment**: `AGENTLESS_EROSION_GATE` and `AGENTLESS_EROSION_LOG`, as `contrib/hooks/agentless_erosion_gate.py:26-28` reads them. `AGENTLESS_EROSION_BIN` has no use in process; the Rust hook ignores it, and the agent guide says so.
- **Steps**:
  - Mode resolution: the repository `erosion_gate` entry, then `AGENTLESS_EROSION_GATE`, then advisory. `off` exits before any work.
  - Run the pair checks in process, with the clone index from `TagsDb` only.
  - Classify the outcome as blocked, advisory, degraded, clean, or skipped. A gap in `OFF_REASONS` (`agentless_erosion_gate.py:60-63`: `no_boundary_globs` and `no_rules_for_language` for `contract_drift`, `no_tag_cache` for `clone_delta`) means the check is off for that file, so it does not make the outcome degraded.
  - Output: advisory `additionalContext` JSON on stdout and exit 0; block with exit 2 and findings on stderr. At most 15 findings.
  - The JSONL record keeps the schema `contrib/hooks/erosion_gate_report.py` reads. `binary` is the current executable. `dropped` lists each requested check that lint does not offer as a pair check (`agentless_erosion_gate.py:663-667`); in process every gated check is offered, so it is empty, and a test pins that.
- **Limits**: the 10 s overall deadline (`DEADLINE_S`) with the 0.5 s log reserve (`LOG_RESERVE_S`) and the 2 s git timeout (`GIT_TIMEOUT_S`, `agentless_erosion_gate.py:36-40`). The Python hook calls lint as a subprocess with no active `Deadline`, so its clone index gets the full 10 s budget. In process the hook passes no call allowance to `walk_budget`, so the budget stays 10 s and the outcomes match. SIGTERM caught (signal-hook): log `degraded`, exit 0; fail open on any internal error.
- **Python source**: `contrib/hooks/agentless_erosion_gate.py` (whole file); spec D8 in `docs/plans/2026-10-09_erosion-checks.md:305-336`.
- **Tests**:
  - Edit emulation against a naive model (proptest), including CRLF majority and the tie.
  - Payload fixtures: valid, malformed, extra fields, and over the cap.
  - Each outcome in all 12 tier-1 languages, and the `no_tag_cache` row giving a clean outcome.
  - The deadline with an injected clock.
  - SIGTERM sent to a child process.
  - The log record schema checked by running the Python report over Rust-made records (development time).

### 13. `agentless` and the switch

- `main` reads argv and `EnvConfig` once, builds `Services` from the real ports, and dispatches to `cli` or `hook`.
- `contrib/hooks/settings-example.json`: the erosion hook entry calls the Rust binary by absolute path, `hook erosion`, with a timeout of 15 s.
- The Python hook script stays for users of the Python package until P4.

## Parity harness

- **Pair-check rows**: the lint goldens are SEARCH/REPLACE candidates, which P1 cannot parse. So `scripts/pair_parity_fixtures.py` runs once under the frozen 0.10.1 build. It writes each golden candidate's (path, old text, new text) pairs and the three pair checks' rows and `observed` lists to `crates/checks/fixtures/`. A Rust test compares the rows after `PyJson`: checks, paths, lines, messages, counts, and `observed` byte for byte. For `clone_delta`, the reported copy site may differ only on the fixtures listed in `crates/checks/fixtures/accepted.toml`, each with the reason (decision 6). Every other field is exact.
- **`--since`**: a script builds a git repository with six commits over the fixtures and runs both builds with `--since HEAD~5 --json`. Before the comparison, it asserts that both builds exit 0 and that each of the three checks reports at least one row. The output must be equal, apart from the receipt line.
- **Hook**: a payload corpus of about 40 Write and Edit calls across the 12 languages runs through both hooks, in three cache states over the same `tags.db`: fresh (0.10.1 `index`), stale (0.10.1 `index`, then one file in ten changed), and absent (an empty `XDG_CACHE_HOME`). Decisions, stdout, stderr, and log records must be equal, apart from `ts`, `latency_ms`, and `binary`.

## Exit gates

- [ ] **Latency**. `scripts/hook_latency.py` uses the overview's baseline method. It runs the Python 0.10.1 hook and the Rust hook interleaved, 30 runs each, on three repositories: a frozen copy of this repository, the mcp-local clone at f17af96, and the dgl Loc-Bench checkout. Each repository runs in the three cache states, which the script seeds with 0.10.1 `index`.
  - On mcp-local, in each cache state: p50 50 ms or less and p95 100 ms or less (proposed; the owner confirms before P1 starts).
  - The other repositories are recorded, with no threshold.
  - Medians, p95, and the commands go in CHANGELOG.md.
- [ ] **Hook outcome suite** passes in all 12 tier-1 languages: Edit emulation, advisory and block output, the `no_tag_cache` row, malformed and extra-field payloads, deadline and SIGTERM, the log record schema, and degraded analysis.
- [ ] **Tokenizer corpus** shows zero differences against pinned CPython 3.13, and the fuzz job runs.
- [ ] **Complexity goldens** (6) and the scb-check snapshot match.
- [ ] **Pair-check parity rows** (with the accepted list), `--since` parity, and hook parity in the three cache states are equal.
- [ ] **Recorded**: binary size and `Flat` memory per node.
- [ ] **Gates**: `cargo xtask layers`, `public-api`, `suppressions`, `lint-tables`, and clippy pass. The erosion gate (pinned 0.10.1) passes on `crates/`. The mutation job (overview rule 11) has run once and set its floor.
- [ ] **Release**: the merge commit is tagged `rust-p1` (overview rule 1). From then on, CI builds the lint gate from that commit (overview rule 2).

## Risks

| Risk | Response |
|---|---|
| The tokenizer differs from CPython on a rare form | The corpus and the fuzz job find it; each mismatch becomes a fixture. The gate needs zero differences. |
| The latency gate fails | Profile before changing design. The likely costs are the repository listing and reading every in-scope file for the clone index (`lint_service.py:359-373`). |
| Histogram pairing differs from SequenceMatcher on a real edit | Counts never change. A different reported copy is accepted (decision 6) only for a fixture on the accepted list, and the move-plus-copy test pins the chosen case. Python's `SequenceMatcher` keeps `autojunk` on: in a new text of 200 or more lines, a line that makes up more than 1% of it leaves the match index, and Histogram has no such rule. A fixture of 200 or more lines with many brace-only lines pins that case. |
| A Python change bumps the `tags.db` schema during P1 to P3 | The reader accepts schema 17 only and reports `no_tag_cache` otherwise, so the hook stays safe. Overview rule 7 forbids the bump without a matching reader change. |
| `Flat` memory is too high on large files | It is about 22 bytes per node (estimate); P1 records it. If needed, drop `Flat` for files over a size and walk the `Tree` instead. |
| The vendored grammar sources do not build on macOS | The D1 build was measured on Linux only. Build them on the macOS CI row before P1 relies on them. The scanners of ruby, scala and swift export globals without a language prefix; none collide among the 22. |
