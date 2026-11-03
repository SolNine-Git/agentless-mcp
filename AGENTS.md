# AGENTS.md

Instructions for coding agents that work in this repository. The README
describes the tools for their users. This file describes how to change them
and how to prove that a change did no harm.

## Commands

```bash
uv sync                                   # the dev group: pre-commit, ruff, codespell
cargo xtask policy
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo insta test --check --workspace --test-runner nextest
cargo deny check
cargo machete --no-ignore crates xtask
cargo xtask static-gates                  # layers, suppressions, lint-tables
cargo xtask ground-truth
cargo xtask public-api
gate="$(uv tool dir --bin)/agentless-mcp" # an installed build, never the build under test
cargo xtask erosion --lint-bin "$gate" --health-bin "$gate"
uv run pre-commit run --all-files         # file hooks, ruff, codespell, commit-stage cargo hooks
```

CI's rust job runs these cargo commands in this order, on Linux and macOS.
Run them before you call a change done. `uv run pre-commit run --all-files
--hook-stage pre-push` runs the pre-push hooks: ruff over the whole tree,
the tests, ground-truth, public-api, erosion and policy. A change to `xtask/`, a manifest, a policy
file, `.pre-commit-config.yaml` or `.github/workflows/ci.yml` also needs
`scripts/gate-selftest.sh "$gate"`, which proves that every gate fails on its
own violation. `cargo xtask` with no task lists every task.

Never silence a linter. Fix the code it points at. `crates/suppressions.toml`
lists each approved suppression, and `cargo xtask static-gates` refuses any
other. A change to a policy file needs a `Policy-Change: <path>` trailer on
its commit. `xtask/src/policy.rs` lists the policy files.

## Where things live

- `crates/` holds the Rust workspace. `crates/agentless` builds the two
  commands, `agentless-mcp` and `agentless-mcp-server`.
  `crates/layers.toml` gives each crate a layer, and a crate depends only on
  crates in lower layers. `cargo xtask layers`, part of `static-gates`,
  enforces the order, and `crates/dependencies.toml` lists each direct
  third-party dependency. Change the design, not the contract, when a
  dependency does not fit.
- `xtask/` holds the development gates and the code generator.
- Every caller-facing string (tool and parameter descriptions, refusals,
  notes) lives in `crates/prompts/data/*.json`. The agent guide lives in
  `crates/app/data/agent-guide.md`. `cargo xtask generate` writes the Rust
  that serves them, and a test fails when the committed output is stale. A
  new message needs a row in `crates/prompts/fixtures/render.json` with
  sample values and the rendered text. The tests in
  `crates/prompts/src/tests.rs` and `xtask/src/generate.rs` count the
  templates.
- `tests/characterization/` holds the fixture repositories and the
  byte-exact goldens of the rendered output, which the Rust tests read. No
  tool writes the goldens. Change one deliberately, never reflexively:
  predict the diff, edit the golden from the output that the failing
  assertion prints, and read the diff before you keep it. A changed map or
  graph golden also changes its cl100k counts in
  `crates/app/fixtures/read/tokens.json`. `scripts/scb_parity_goldens.py`
  writes `goldens/complexity` with scb-check.
- `crates/*/fixtures/` hold recorded reference outcomes. No generator writes
  them. Never edit an expected value to make a test pass: fix the code. A
  deliberate behavior change edits the value, and the commit message and
  `CHANGELOG.md` name it.

## Proving a change did no harm

Read [docs/analysis/benchmark-methodology.md](docs/analysis/benchmark-methodology.md)
before you quote a benchmark figure or change anything that ranks, resolves
or renders. Its section "The release regression gate" is the procedure a
release follows:

1. The suites and pre-commit, with every golden diff predicted.
2. The Loc-Bench retrieval tier from a frozen copy of the build, with
   `ranked_files` byte-diffed against the stored arm for the base commit.
3. The surface diff: both builds answer the same queries over the Loc-Bench
   checkouts, with tier transitions, a loss count, a prediction written down
   first, and a sample of changed lines read in the source.
4. The exposure check before any paid agentic run.

Two rules from that document prevent the integrity incidents it records:

- Never point a benchmark at the shared working tree. Use a detached
  worktree or a copy without `.git`, `.venv` and `target`. An edit during a
  run splits the arm across two builds.
- Match the instrument to the claim. The retrieval tier sees only the ranked
  file list. A change to fan-in tiers, `explain`, `cycles`, `history` or
  symbol packing needs the surface diff, and a null from a tier that cannot
  see the change is not evidence about it.

## Releases

A release takes two commits on its branch: "Prepare X" (the changelog entry,
the version in the root `Cargo.toml` under `[workspace.package]`, and
`Cargo.lock`), then "Release X: date the changelog" once CI is green. Cargo
writes a pre-release as `0.11.0-rc.1`, and maturin publishes it as
`0.11.0rc1`. The pull request merges with a merge commit, and a lightweight
tag `vX` goes on the merge.

Every push runs `.github/workflows/release.yml`. It runs the release-profile
checks, builds the 4 wheels and the sdist with maturin from `packaging/`, and
checks the installed wheels. On a `v*` tag, its `publish` job then waits
until a reviewer approves the deployment to the GitHub environment `pypi`.
The job uploads by trusted publishing: PyPI accepts the workflow's OIDC
token, and no API token is used. A release is the tag plus the PyPI upload.
There are no GitHub releases. After the upload, install the release with
`uv tool install --refresh "agentless-mcp==X"`. Without `--refresh`, uv can
read a stale index and report that no such version exists.

`.github/gate/requirements.txt` pins the CI gate binary by hash. It moves on
purpose, not with each release. Record each claim in `CHANGELOG.md` with the
measurement that earns it.
