# Python import resolution: the measurement record

This record holds the measurements of the three Python import rules of f9afd14, 225f154 and
d650012 (the CHANGELOG entry under Unreleased / Fixed names them). They follow the release
regression gate of [the methodology](benchmark-methodology.md): the suites, the retrieval tier as a
byte-diff, and the surface diff. Each figure is **measured** (with the command and n) or
**derived** (with the method). A "row" is one import statement's edge to one file, as the graph
stores it; a refs "site" is one refs line, keyed by (path, line).

- Rule 1: an `if` condition that the file's own imports bind to TYPE_CHECKING (`t.TYPE_CHECKING`
  after `import typing as t`) is a TYPE_CHECKING block. It changes only `Import.runs`
  (`cycle_dodge` reads it) and the clone walk (`clone_delta`, the health clone share).
- Rule 2: in a Python file whose directory holds `__init__.py`, an absolute module without a dot
  is not tried beside the importer. It changes the statement's target, so it changes import
  edges and the bindings that refs and explain read.
- Rule 3: `from P import N`, where every N is a module and the importer is inside package P, has
  no row to P/__init__.py. The bindings do not change, so no refs or explain tier changes.

The builds: the base is the release build of 6dce8e5 (the tree of main 0193eb1, which is
deployed; agentless-mcp sha256 `c9b037ea...`), the tip is the release build of f5102e4 (the code
of d650012 and a guide text change; `9be56585...`). `sha256sum -c` passed for both. Every run had
an absolute `XDG_CACHE_HOME` and the gate and erosion logs under the work directory, and no output
held "is relative and was ignored". The work records are under `/var/tmp/claude-rust-port/cdr`
(ticket CDR: the rules and the U3 rerun) and `/var/tmp/claude-rust-port/cdm` (ticket CDM: this
record); each `status.md` holds each prediction, written before its run.

## The U3 rerun

U3 measured `cycle_dodge` on 60 blind findings (record:
[2026-11-05_cycle-dodge-measurement.md](2026-11-05_cycle-dodge-measurement.md)), and 10 false
alarms came from the three faults that the rules fix. CDR ran `lint --repo <copy> --since <root>
--only cycle_dodge --json` with the release build of d650012 on the 14 snapshot copies, read-only
(measured, 14 runs, each exit 0, 0.07 to 0.67 s, at most 529 MB):

- Rows: 1,638 -> 1,591 (1,567 the same, 24 with another chain, 47 gone, 0 new). No snapshot gains
  a row.
- The 60 sampled rows, keyed by path and line: the 10 false alarms are gone; the 50 others remain
  with the same chain.
- Of the 83 local imports with a comment that names a circular import, 56 are found (59 before).
  The 3 lost are the pilot's local-only and type-only rows whose chain held a removed edge.

Between d650012 and the tip of this record (f5102e4), only a test alias, the CHANGELOG and the
guide text change. The tip's build gives byte-equal `--json` output (`cmp`) on the pytest-dev and
sympy copies (measured, 2 runs, exit 0).

## The suites

At f5102e4, every command of [AGENTS.md](../../AGENTS.md) exits 0 (measured; 1,630 tests), with
`uv run pre-commit run --all-files` and `--hook-stage pre-push`. The first pass failed the policy
and erosion steps on a missing `origin/main` in the work clone (an environment fault); after a
fetch of main (6b074a6) they pass. `scripts/gate-selftest.sh` is not required: no change touches
`xtask/`, a manifest, a policy file or the CI configuration.

## The prediction model

`scripts/shapes.py` under the CDM work directory is a Python copy of the old and the new resolver
(`resolve_import_target`) and of `bind_statement`, written from the source. On the graph corpora
`ext_python` and `ext_communities` it gives exactly the 6 and 42 rows that d650012 removes and 0
rule-2 rows. Over the 144 Loc-Bench checkouts (derived): 640 rule-2 rows, 8,620 rule-3 rows (764
of them a row to the importer itself, which gives no edge) and 489 binding changes. 123 of the 150
instances hold a changed row or binding (the set S); the other 27 hold none. Over the 14 U3 copies:
252 rule-2 rows, 1,486 rule-3 rows and 169 binding changes; all 14 are in S.

## The retrieval tier

The harness ran each frozen binary with `loc-bench run --agentless-bin`, seeded and no-seed, 150
instances per arm (measured; 25 to 26 s per arm). After each run, 0 files of the harness were newer
than the run's stamp.

- The base reproduces the stored arm of the last resolver change (RSM's tip, a4207bd): 0 of 150
  instances differ in `ok`, `ranked_files`, `resolved_seeds` and `unresolved_seeds`, in each arm.
  Main gained PR #71 between them (`cycle_dodge`, `Import.runs`, `import_chain`); it changes no
  resolver rule, file-graph weight or rank code.
- The tip against the base: `ranked_files` differ in 97 of 150 seeded and 110 of 150 no-seed
  instances. Each one is in S; 0 of the 27 instances outside S changed. The file set changed in 48
  and 63 lists; the others changed order only. `ok`, `resolved_seeds` and `unresolved_seeds`: 0
  differ.
- recall@10 moves in 1 seeded instance and 2 no-seed instances. webcompat-2731 rises 0.1 -> 0.2
  in both arms (the gold file webcompat/api/endpoints.py moves from rank 12 to 9). No-seed
  django-ninja-1349 falls 1.0 -> 0.0: its one changed row is ninja/params/functions.py:8 `from
  ninja.params import models` (rule 3), so ninja/params/models.py rises from rank 12 to 10 and the
  gold file ninja/responses.py falls from 10 to 12.

The paired bootstrap (10,000 samples, seed 20261105; mean delta, 95% interval, instances better
and worse), measured:

| Metric | seeded | no-seed |
|---|---|---|
| recall@10 | +0.000667 [0, +0.002000] (1, 0) | -0.006000 [-0.020000, +0.002000] (1, 1) |
| MAP | +0.003689 [-0.002288, +0.012378] (14, 6) | +0.004009 [-0.002823, +0.013094] (21, 9) |
| nDCG@10 | +0.003353 [-0.001763, +0.010132] (7, 2) | +0.001053 [-0.006332, +0.008861] (12, 6) |
| recall@5 | +0.005000 [-0.005000, +0.020000] (1, 1) | +0.006667 [-0.003333, +0.023333] (2, 1) |
| acc_any@1 | +0.006667 [-0.013333, +0.033333] (2, 1) | 0 [-0.026667, +0.026667] (2, 2) |
| acc_any@5 | 0 [-0.020000, +0.020000] (1, 1) | 0 [-0.020000, +0.020000] (1, 1) |

Every interval holds 0. More instances are better than worse on every precision metric (derived
cause: most removed rows end at a package's `__init__.py`, so those entry files fall and other
files rise).

## The surface diff

`surface_dump.py` of `v0.11.0rc1` (dump mode) and `scripts/surface_cmp.py`, base against tip, 8
workers, a 120 s timeout, on copies of the checkouts. The Loc-Bench targets are the 150 of the P2
surface diff (1,206 queries). The U3 targets are 2 per copy (picked before the dumps, seed
20261105): one with up to 10 symbols of the files that a changed binding or a removed row names,
and one with 10 random module-level symbols; each symbol gets `refs --json` and `explain`, and each
target a `map` (focus: the files of its first 2 symbols) and `cycles` (540 queries). Both builds
listed identical `find-symbol` frames; no call timed out, changed its exit code or changed its
stderr.

| Surface | Loc-Bench changed / calls | U3 changed / calls |
|---|---|---|
| cycles | 106 / 150 | 28 / 28 |
| explain | 21 / 228 | 130 / 242 |
| map | 116 / 150 | 28 / 28 |
| refs | 1 / 678 | 10 / 242 |

- The model named each changed answer before the dumps, except 3 Loc-Bench answers (below). The
  U3 changed answers equal the predicted sets on all four surfaces.
- cycles: in 83 Loc-Bench answers the note "N import statements of this repository's own modules
  did not resolve" counts more statements (rule 2: a module that resolved beside the importer, or
  to the importer itself, no longer resolves); 35 answers change only that note. The cycle count
  falls in 32 Loc-Bench answers and in 18 U3 answers (9 copies, such as ansible 4 -> 2 and
  scikit-learn 8 -> 6). It rises in 7 Loc-Bench answers and in 2 U3 answers (sphinx-doc 7 -> 8):
  a component that lost an edge splits in two. Both sets equal the model's sets.
- refs: `total` is equal in every answer. Sites that move, keyed by (path, line): Loc-Bench 2
  (resolved-via-import -> name-only-ambiguous: `pandas.DataFrame(` in modin/conftest.py, where
  `import pandas` had bound to modin/pandas/__init__.py); U3 26 (resolved-via-import -> unique 24,
  -> name-only-ambiguous 2, all in sympy/plotting/pygletplot). No site moves up from a name-only
  tier, and no site enters or leaves an answer.
- explain: 0 fan-in or fan-out rows gain a binding tier from a name-only tier. 2 Loc-Bench rows
  appear at resolved-via-import because rows above them left a capped list (they existed in both
  builds); U3 fan-in rows resolved-via-import -> unique 13 and -> name-only-ambiguous 1, one fan-out
  row -> unique. The other explain changes are in the imports section: rows to a package entry
  that rule 3 removes, and declared imports that rule 2 no longer resolves.
- The loss count of `surface_cmp.py`: 33 (Loc-Bench: the 32 cycle counts and modin-4391 `explain
  DataFrame`, fan-in 492 -> 489) and 18 (U3: the cycle counts).
- Truncated answers: 18 and 18 (Loc-Bench), 7 and 7 (U3). One changed answer is truncated in both
  builds (modin-4391 `refs DataFrame`).

### The read sample

45 changed lines, read in the source (seed 20261105): per class, 10 drawn from the refs sites that
move (28 in all), the explain lines that a diff adds or removes and that name a source line (804),
and the modelled rows in a cyclic component of a changed cycles answer (6,832). A fourth class of
15 rule-2 rows (of 892) was added after the first three were read, because they held only 12 rule-2
lines; it is post hoc. map is out of the sample: its changed lines are rank values.

| Class | new answer right | old answer right | neither |
|---|---:|---:|---:|
| refs sites | 0 | 10 | 0 |
| explain lines | 9 | 1 | 0 |
| cycles rows | 8 | 1 | 1 |
| rule-2 rows (post hoc) | 14 | 1 | 0 |
| all | 31 | 13 | 1 |

- New answer right: a rule-3 row to a package entry from a module inside the package (`from . import
  cbook` in matplotlib's backends, `from keras.src import backend`, `from numba.core import types,
  ...`); a standard-library or third-party module that resolved to a project file of the same name
  (`import numpy as np` beside keras/src/backend/torch/numpy.py, `import responses` beside
  moto/core/responses.py, `from typing import Type` beside sphinx/util/typing.py, ansible's `import
  tempfile` under `from __future__ import absolute_import`); and 3 explain rows that only show
  because rows above them left a 20-row list.
- Old answer right, rule 2 (12 lines): the sympy snapshot declares Python 2.6 to 3.5, and
  sympy/plotting/pygletplot imports its siblings by implicit relative import (`from plot_object
  import PlotObject`), which only Python 2 resolves to the sibling. The same holds for the
  webcompat checkout of webcompat-2731, which runs on Python 2.7 (`from helpers import add_csp` in
  webcompat/views.py, 23 rows). One moto line is the `else` branch of ANTLR's `if "." in
  __name__:` template, which runs only as a top-level module (its relative branch keeps the edge).
- Old answer right, rule 3 (1 line): sentence_transformers/losses/MegaBatchMarginLoss.py:9 `from
  sentence_transformers import SentenceTransformer, util`. The package entry re-binds the name
  `SentenceTransformer` to the class (`from sentence_transformers.SentenceTransformer import
  SentenceTransformer`, __init__.py:20), so the statement reads the entry's attribute and depends
  on how far __init__.py has run. Derived count of this shape (a member N that P/__init__.py binds
  to another object, by Python's ast): 92 of 7,856 rule-3 rows on Loc-Bench (76 in the three
  sentence-transformers checkouts) and 3 of 1,317 on U3.
- Neither: keras/api/__init__.py:15 imports a submodule of its own package; the row ends at the
  importer, so no build draws an edge.

## Where the predictions missed

- The retrieval tier: the point guesses were about 80 seeded and 100 no-seed changed lists; measured
  97 and 110. No-seed recall@10 was predicted in [0, +0.02]; measured -0.006 (django-ninja-1349,
  above). The bound held: every changed list is in S.
- The surface diff, Loc-Bench: 2 cycles answers (xclim-477, scapy-4586) keep their component
  members and change only the printed chain, because an edge inside the component left; the model
  compared member sets. modin-4391 `explain DataFrame` lost 3 fan-in rows through the `import
  pandas` module binding; the explain predictor read only member bindings. The clause "the 27
  instances outside S give 0 changed answers" failed for ragas-1627 `cycles` (the note count 35 ->
  36: `from langsmith import Client` in src/ragas/integrations/langsmith.py had resolved to its own
  file), which the cycles clause of the same prediction named. map changed in 116 answers; the
  guess was about 100.
- The prediction of the sample verdicts: rule 3 was expected to be right in every read line. One
  line (the re-bound member above) was not.

## The re-bound member

The read sample above found one line where rule 3 is wrong:
sentence_transformers/losses/MegaBatchMarginLoss.py:9. Commit a0c7dd2 adds an exception to rule 3.
The work records are under `/var/tmp/claude-rust-port/r3k` (ticket R3K); its `status.md` holds each
prediction, written before its run. The base is the deployed release build of b3969db
(agentless-mcp sha256 `67e155f3...`), and the tip is the release build of a0c7dd2 (`2a9c3ebb...`).
Every run had an absolute `XDG_CACHE_HOME` and the gate and erosion logs under the work directory.

### The rule

For an importer inside package P and `from P import N...` where every N is a module, the row to
P/__init__.py stays when P/__init__.py binds one of the N at module level to an object that is not
the module P/N:

- a symbol of P/__init__.py named N with no parent: a def, a class, an upper-case constant or a
  type alias, in any module-level block;
- an import that runs at load (not in a function, not in a TYPE_CHECKING block) and is not a star
  import, whose local name is N and whose object is not the file P/N. `from .N import N` binds the
  class; `from . import N`, `from P import N` and `import P.N as N` bind the module.

The rule reads the facts of P/__init__.py, so nothing parses again, and the bindings that refs and
explain read do not change. `cycle_dodge` and `cycle_delta` build their import graphs from reduced
facts; both now keep the symbols of a package entry, so every import graph reads the same rule. The
rule does not read a lower-case `N = ...` (the facts make only an upper-case module assignment a
symbol), a star import or a module `__getattr__`. A def, class or constant in a TYPE_CHECKING block
counts, because a symbol carries no load mark.

The definition of the earlier count (`shadowed.py`: the top-level statements of P/__init__.py, by
Python's ast) differs: the rule also reads nested blocks, class bodies and `import` statements,
counts only upper-case assignments, and decides "the module itself" by the resolved file, not by
the module text. Derived (a Python model of the rule over the same rows): the rule keeps 92 of
7,856 rule-3 rows on Loc-Bench (in 10 checkouts) and 3 of 1,317 on U3 (in sympy), the same rows as
`shadowed.py`. An import at top level re-binds each one: on Loc-Bench 71 `from X import N` from
another module and 21 `from .N import N`, on U3 3 `from .N import N`. No def, class or assignment
form occurs.

### The U3 rerun of the exception

`lint --repo <copy> --since <root> --only cycle_dodge --json` with the tip build on the 14 snapshot
copies, read-only (measured, 14 runs, each exit 0, 0.07 to 0.61 s, at most 541 MB). The prediction:
the 13 copies without a restored row give byte-equal output, sympy keeps every row, and the 10
false alarms stay gone. The result: all 14 outputs are byte-equal (`cmp`) to the rerun of d650012
above: 1,591 rows, none new, none gone and none with another chain. The 10 false alarms stay gone,
and the 50 other sampled rows remain with the same chain. The 3 restored rows of sympy close no new
chain.

### The retrieval tier of the exception

Seeded and no-seed, 150 instances per arm (measured; 23 to 24 s per arm). After each run, 0 files
of the harness were newer than the run's stamp.

- The base against the stored tip of this record (f5102e4): 0 of 150 instances differ in `ok`,
  `ranked_files`, `resolved_seeds` and `unresolved_seeds`, in each arm. Main changed no graph, map
  or rank code between them.
- The prediction: `ranked_files` change only in the 10 instances whose checkout holds a restored
  row (derived). The result: 5 of 150 seeded and 6 of 150 no-seed lists change, each one of the 10;
  `ok`, `resolved_seeds` and `unresolved_seeds`: 0 differ.

The paired bootstrap (10,000 samples, seed 20261106; mean delta, 95% interval, instances better
and worse), measured:

| Metric | seeded | no-seed |
|---|---|---|
| recall@10 | 0 [0, 0] (0, 0) | 0 [0, 0] (0, 0) |
| MAP | -0.000111 [-0.000333, 0] (0, 1) | +0.000101 [-0.000333, +0.000635] (1, 1) |
| nDCG@10 | 0 [0, 0] (0, 0) | +0.000215 [0, +0.000646] (1, 0) |
| recall@5 | 0 [0, 0] (0, 0) | 0 [0, 0] (0, 0) |
| acc_any@1 | 0 [0, 0] (0, 0) | 0 [0, 0] (0, 0) |
| acc_any@5 | 0 [0, 0] (0, 0) | 0 [0, 0] (0, 0) |

matchms-backup-187 loses MAP (-0.0167 in both arms): the importer of its restored row,
matchms/exporting/save_as_mgf.py, enters the list at rank 19, and the gold file
matchms/filtering/derive_inchi_from_smiles.py falls out from rank 20. sentence-transformers-3035
gains in the no-seed arm (MAP +0.0317, nDCG@10 +0.0323).

### The surface diff of the exception

The tools, targets and queries of the surface diff above, base against tip, on new copies of the
checkouts (1,206 Loc-Bench and 540 U3 queries). Both builds listed identical `find-symbol` frames;
no call timed out, changed its exit code or changed its stderr.

| Surface | Loc-Bench changed / calls | U3 changed / calls |
|---|---|---|
| cycles | 5 / 150 | 0 / 28 |
| explain | 0 / 228 | 0 / 242 |
| map | 7 / 150 | 0 / 28 |
| refs | 0 / 678 | 0 / 242 |

- The prediction: no change outside the 10 instances; no refs or explain change; one more cycle in
  each of sentence-transformers-3035, -3073, -3076, cupy-3730 and matchms-backup-187; at most a
  printed chain in numba-9757; map changes only in the 10. The result meets each clause. The new
  cycles are the model's components: the 5 files of sentence_transformers/readers,
  cupyx/scipy/special/__init__.py with polygamma.py, and matchms/__init__.py with
  matchms/exporting/__init__.py and save_as_mgf.py. numba-9757 does not change.
- refs: every site keeps its tier, keyed by (path, line). The loss count is 0. Truncated answers:
  18 and 18 (Loc-Bench), 7 and 7 (U3); no changed answer is truncated.

The read sample (seed 20261106): the refs and explain pools are empty, so the sample is 20 of the
74 restored rows in a component of a changed cycles answer, read in the source. The new answer is
right in 20 and the old in 0. All 20 are in the sentence-transformers checkouts: 17 `from
sentence_transformers import SentenceTransformer`, where the entry binds the class
(`from sentence_transformers.SentenceTransformer import SentenceTransformer`, __init__.py:20), and
3 `from . import InputExample` in sentence_transformers/readers, where readers/__init__.py:3 binds
the class before those modules load. 5 of the 20 run in a function, and they read the entry's
attribute when called.

### Where the predictions of the exception missed

- The retrieval tier: the point guesses were 8 of 10 seeded and 9 of 10 no-seed changed lists;
  measured 5 and 6. The bound held.
- The surface diff: map changed in 7 of the 10 instances; the guess was 8.
- The first clippy run refused two functions (nesting, and six arguments); the commit has the
  fixed code, and every proof above is of that code.
