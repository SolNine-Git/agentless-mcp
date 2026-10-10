# AGENTS.md

Instructions for coding agents that work in this repository. The README
describes the tools for their users; this file describes how to change them
and how to prove a change did no harm.

## Commands

```bash
uv sync --all-extras                 # the dev environment, mcp and tokens extras included
uv run pytest -q                     # unit and characterization suites
uv run pre-commit run --all-files    # ruff, mypy, codespell, import-linter, deptry
```

Run both before you call a change done. Never silence a linter or the type
checker; fix the code it points at.

## Where things live

- `src/agentless_mcp/` has four layers, enforced by the import-linter
  contracts in `pyproject.toml`: `adapters -> application -> core -> prompts
  -> util`. The CLI and MCP adapters are independent of each other. Change
  the design, not the contract, when an import does not fit.
- Every caller-facing string (tool and parameter descriptions, refusals,
  notes) lives in `src/agentless_mcp/prompts/*.json`, with a typed field in
  `prompts/__init__.py`. `tests/unit/test_prompts.py` formats each message
  with a sample of its placeholders, so a new message needs a sample there.
- `tests/characterization/` holds byte-exact goldens of the rendered output.
  Regenerate them deliberately, never reflexively: predict the diff, run the
  module's `regenerate()`, and read the diff before you keep it.

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
  worktree or a copy without `.git` and `.venv`; an edit during a run splits
  the arm across two builds.
- Match the instrument to the claim. The retrieval tier sees only the ranked
  file list. A change to fan-in tiers, `explain`, `cycles`, `history` or
  symbol packing needs the surface diff, and a null from a tier that cannot
  see the change is not evidence about it.

## Releases

A release takes two commits on its branch: "Prepare X" (the changelog entry,
the version in `pyproject.toml`, and `uv.lock`), then "Release X: date the
changelog" once CI is green. The pull request merges with a merge commit,
and a lightweight tag `vX` goes on the merge. A release is the tag plus the
PyPI upload; there are no GitHub releases. Record each claim in
`CHANGELOG.md` with the measurement that earns it.
