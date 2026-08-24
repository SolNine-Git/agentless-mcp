> Archived 2026-08-24: superseded by
> [`docs/analysis/benchmark-methodology.md`](../benchmark-methodology.md), which
> carries forward the guardrail rules this post-mortem proposed.

# Post-mortem: why a 9-phase audit found 335 defects a review had not

Written 2026-08-23, from the `audit-remediation` branch, after stages 0-6a of
the remediation. Two questions: why these defects surfaced now, and what to
change in the gates so the next batch surfaces before an audit has to look.

The counts below are the ones that stood at stage 6a. The remediation closed at
**337** findings (316 fixed, 12 superseded, 9 not established); the two added
after this was written do not change any argument here, and the number is left
as it was measured rather than restated, so the write-up and its evidence agree.

## What the numbers are, and what they are not

The audit segmented `src/agentless_mcp` into 25 blocks, ran a 7-pass
adversarial protocol per block in isolation, then a 3-persona validator panel,
a cross-block synthesis, and an adversarial pass over its own output. It
produced **335 findings**: 1 Critical, 60 High, 143 Medium, 131 Low.

Two calibrations before drawing conclusions from that number.

**The method is designed to over-generate.** Twenty-five independent passes
with no shared context will each report what they see, including the same
thing twice under different names and a long tail of one-reviewer opinions.
**131 of the 335 are single-reviewer Lows** -- naming, docstring wording,
redundant guards. They are worth fixing and they are not defects in the sense
the word usually carries. A count that mixes them with a Critical is a count
that means very little on its own.

**The codebase was not in bad shape.** No import cycles, an enforced
import-linter layer contract, 1,780 tests at 91.8% line / 85.8% branch, zero
mock-return assertions, every module carrying a real docstring explaining its
design decisions. The defects are subtle *because* the gross ones were already
handled. That is the finding, not an aside: **this codebase verified structure
rigorously and behaviour informally**, and every gate it had was a structural
gate.

## Why a review did not find them

Sorting the fixed findings by *what it would have taken to see them* gives six
categories. None of them is "the reviewer was not careful enough".

### 1. They needed a reproduction, not a reading

The Critical is the clearest case. `install_fingerprint` read:

```python
record = installed.read_text("RECORD") or ""
```

Reading that, `or ""` looks like ordinary defensiveness. Running it is a
different experience: `PathDistribution.read_text` suppresses `FileNotFoundError`
and returns `None`, so `or ""` yields `sha256(b"")` -- a *valid, different*
fingerprint. The server saw a fingerprint change and restarted itself. The
`except (PackageNotFoundError, OSError)` arm on the next line could never fire.

Measured: RECORD present `0.5.0:0c43d58230197bdb`, absent
`0.5.0:e3b0c44298fc1c14`.

Same category: `cycles --limit 0` reporting "no import cycles" for a repository
that has one; a filename containing a newline rendering a byte-identical
structural row below the untrusted-content banner; an ambient `GIT_DIR` making
the receipt carry *another repository's* HEAD.

Every one of these is invisible to reading and obvious in one command.

### 2. The fixture did not exist, so no test could fail

`core/extractor.py` was the weakest module at 82% line / 67% branch, and the
reason was not thin tests. There was **no `.rs`, `.c`, `.h`, `.cpp` or `.java`
fixture anywhere in the repository**. Two hand-written extractors and one
tier-1 generic-walker language ran against zero inputs.

A coverage report says "82%". It does not say "three of your tier-1 languages
have never been executed". Nothing in the gate stack asked whether the set of
languages the tool advertises matches the set of languages a fixture exercises.

Writing those five fixtures found a defect the audit itself could not have
seen: tree-sitter-rust nests `async` inside a `function_modifiers` node, so
`_extract_rust_function`'s check on direct children never fires and **no Rust
function has ever been reported async**.

### 3. They needed counting, and nobody counted

- `capability_service._caps()` said it returned "every public bound in force".
  It returned 8 of 23.
- `DEFAULT_MAX_NODES` meant 40 in one module and 200 in another;
  `DEFAULT_MAX_EDGES` meant 40 and 600. Both spellings reached `--help`.
- The remediation plan named 8 emission sites in the renderer that needed
  escaping. There were 25.

Each is a claim about a set. A reviewer reads the claim and the nearest few
members and moves on; the claim is only falsified by enumerating.

The test written to enforce the first of these immediately found a third
constant collision the audit had missed -- `DEFAULT_MAX_FILES` was 10 in
`map_service` and 20000 in `fslimits`, and `capability_service` was importing
the second under an alias. **An alias like that is what a collision looks like
after somebody has already tripped over it and worked around it locally.**

### 4. The environment was never varied

- `FASTMCP_MASK_ERROR_DETAILS` was left at FastMCP's default. Measured: 243
  MCP tests pass with it unset, 22 fail with it set to `true`, because masking
  replaces this package's refusal wording -- the wording an agent reads to
  correct its own call.
- `XDG_CACHE_HOME=relcache` put scratch worktrees *inside the repository being
  analysed*.
- `GIT_DIR`, `GIT_INDEX_FILE` overrode the `-C` every git call depends on.
- `Path.resolve()` raises `RuntimeError` on a symlink loop on the declared 3.10
  floor and returns a nonexistent path from 3.13. The handler caught neither,
  so the same input was an untyped crash on one supported interpreter and a
  silent acceptance on the other.

The suite ran in one environment, on one interpreter, and passed. Every one of
these is a *configuration* the code reads and the tests never varied.

### 5. Concurrency, where the obvious test does not reproduce

Two of three background registries had check-then-set races. The instructive
part is what it took to write a test that fails against the unfixed code.

A barrier at the *call site* -- twelve threads released at once into
`start_auto_warm` -- **does not reproduce it**. The function is short enough
that the first caller finishes inside one GIL slice and the others never
observe the empty registry. The test passes against the broken code.

The reproduction needs a barrier *inside the probe*, at the point where the
real code does filesystem work and releases the GIL, so all twelve callers are
provably between the check and the set at the same instant. A reviewer who
writes the obvious concurrency test concludes the code is fine.

### 6. Zombie tests: a test that asserts the defect as intent

The Critical survived review because of this one:

```python
def test_a_missing_record_file_is_still_a_fingerprint(...)
```

It asserted the broken behaviour, it was green, and it contradicted the
module's own docstring ("callers must treat `None` as wait, never as
changed"). Anyone reviewing that module saw a passing test named for the
behaviour and moved on.

`test_parser_is_memoized` was the same shape: it pinned a process-wide mutable
`Parser` shared across four parse sites and a background thread as if it were a
design decision.

**A green test does not mean the behaviour is right. It means somebody wrote
down what it does.** Nothing in a lint stack can tell those apart -- but a
review pass that reads each test against its module's docstring can.

## The systemic pattern behind all six

The audit named it SYSTEMIC-1: **nine modules state a safety invariant in a
docstring and enforce it with nothing.**

- `selfrestart`: "callers must treat `None` as wait, never as changed" -- and
  `or ""` guaranteed they never saw `None`.
- `cache`: "a read command never fails because of a cache" -- and the three row
  readers had no exception handler at all.
- `sandbox`: "the scratch is never inside the target repository" -- and nothing
  checked.
- `patch_service`: "a write that fails part-way raises rather than leaving a
  prefix" -- true, and untested at the multi-file granularity where it matters.
- `capability_service`: "every public bound in force" -- 8 of 23.
- `render`: the output contract an agent parses -- **no test file of its own**;
  its 98% coverage was incidental, through service tests and goldens.

The codebase's gates enforce *structure*: import-linter proves the layer
contract, ruff and mypy prove types and style, deptry proves dependency
hygiene. All excellent, all green, and none of them can express "this docstring
makes a promise; here is the gate that keeps it".

## What to change

Ordered by what each would have caught, most first.

### A. Make a stated guarantee name its gate

The highest-leverage change and the cheapest. Adopt the rule, then enforce it
in review: **a docstring that states an invariant must name the test, lint
contract or startup assertion that holds it.** A guarantee that lives only in
prose is a comment.

This is a prompt/checklist change, not a tool. It would have caught the
Critical, the cache read path, the sandbox scratch, and the capability
inventory -- four of the six categories above.

*Possible automation:* a check that flags docstring sentences matching
`never|always|must|guarantees|cannot` in `src/` where the module has no
corresponding test module. Crude, but it produces a review list.

### B. Coverage that follows subprocesses, and a coverage *shape* gate

Subprocess coverage was configured in stage 0 and immediately corrected the
audit's own numbers: `__main__.py` read 0%, actually 80%; `cli/main.py` 76%,
actually 89%.

Beyond the percentage, add gates on coverage *shape*:

- **Every advertised language has a fixture.** Assert
  `set(SUPPORTED_EXTENSIONS.values()) == set(languages a fixture exercises)`.
  This is ten lines and it is the entire content of category 2.
- **Every module that renders agent-facing output has its own test module.**
  `render.py` had none.
- **Fail on any file below a branch-coverage floor**, not just on the total.
  The total was 85.8% while the extractor sat at 67%.

### C. Vary the environment in CI

A second CI job that runs the suite with the knobs moved:

```
FASTMCP_MASK_ERROR_DETAILS=true
XDG_CACHE_HOME=<relative path>
GIT_DIR=<an unrelated repository>
TZ=<something exotic>   LANG=C
```

and the matrix pinned to **both ends of the declared Python range**, not just
the newest. The 3.10-vs-3.13 symlink divergence is only visible that way.

Anything that reads `os.environ` in `src/` is a candidate for this list. That
set is enumerable -- a lint rule could produce it.

### D. Treat "the tool advertises N" as a testable claim

Every place the code makes a claim about a *set* deserves a reflective test
rather than a hand-maintained list:

- the bounds inventory (now gated -- and it found a collision on its first run)
- the numeric CLI surface (now pinned)
- the escaping seam (now gated by reflection over `dataclasses.fields`, which
  is why a field added later is covered without anyone remembering)

The pattern that works: **enumerate by reflection, assert against the claim.**
A list maintained by hand is a claim that decays.

### E. A review pass that reads tests against docstrings

Specifically to catch zombies. For each test module, ask: does this test assert
what the module's docstring says it does, or what the code currently happens to
do? The two Criticals were both protected by a green test.

This does not automate. It is a prompt for a review agent, and it is worth one
dedicated pass.

### F. Reproduce, do not read

The single highest-value change to a review prompt. **A finding is not a
finding until a command demonstrates it, and a clean bill of health on a
behaviour nobody ran is not evidence.**

Concretely, for this codebase: build a scratch repository with a hostile
property (a newline in a filename, an import cycle, a symlink loop, a
non-UTF-8 file) and run every command against it. That sweep is seven lines of
shell and it covers most of category 1.

The remediation itself is the evidence for this. Every fix in the concurrency
stage was checked against the *unfixed* code first, and one test that looked
correct turned out not to reproduce at all -- the call-site barrier in category
5. Without that check it would have shipped as a passing test proving nothing.

## The honest limits of this write-up

- **The audit is not complete either.** Working the fixes turned up five
  defects it missed: the Rust `async` handler, C/C++ includes never resolving
  to a repository file (the include graph was empty on every repository), C
  prototypes yielding no symbols, the third constant collision, and a second
  escaping sink in `core/treewalk`. An audit is a sample.
- **One of the audit's own findings did not reproduce** and was demoted to
  "not established" -- the `XDG_CACHE_HOME` patch-misplacement half. Reviewers,
  including adversarial ones, generate false positives at a rate worth
  measuring.
- **One of the plan's requirements was wrong.** It required a cache generation
  bump with the stable-id fix, on the assumption that the collision ordinal was
  persisted. It is not -- it is re-derived on every read. Bumping would have
  discarded every user's index to fix nothing.

All three argue the same way: **verify the claim, including the claim in the
review.**
