# Rust scope analysis: the measurement record

This record holds the measurements of the Rust scope analysis (rules R1 to R5) in the order of the
measurement plan in [the specification](../plans/2026-11-04_rust-scope-analysis.md) (D6, the release
regression gate of [the methodology](benchmark-methodology.md)). Its figures replace the prototype
figures of the specification's section 3. Each figure is **measured** (with the command and n) or
**derived** (with the method). "Site" is one identifier occurrence; "row" is one refs line, keyed
by (path, line).

The builds: the base is the release build of 81d5a7e (agentless-mcp sha256 `d83b456b...`), the tip
is the release build of a4207bd (`5d5584ec...`). Both were frozen before this work; `sha256sum -c`
passed for both. Every run had an absolute `XDG_CACHE_HOME` and the gate and erosion logs under the
work directory, and no output held "is relative and was ignored". The work records are under
`/var/tmp/claude-rust-port/rsm` (ticket RSM); its `status.md` holds each prediction, written before
its run.

## Binding-tier sites, by rule

The instrument: a4207bd plus only the dump hook of the prototype (`cycles` writes one row per Rust
identifier occurrence when `AGENTLESS_RSC_DUMP` is set), built as a measurement binary and never
committed. The hook classifies each site with the prototype's syntax classifier and reads the role
that the build stores and the tier that the build's resolver gives. With the hook off, the binary
answers byte-equal to the tip build (`cmp` on `refs --json` of `verdicts.rs::record`, `explain` of
`verdicts.rs::vote_candidate` and `cycles` on the 3 trees: 5 of 5 equal). The base side is the
specification's dump of 81d5a7e. The trees: this repository at 81d5a7e, ripgrep-2576 and
tokio-6752. Sites are joined by (path, line, name, class).

Binding-tier sites (same-file plus resolved-via-import), measured:

| Tree | base | prototype (spec 3.4) | tip | removed | gained |
|---|---:|---:|---:|---:|---:|
| this repository | 44,282 | 38,967 | 38,897 | 5,385 | 0 |
| ripgrep-2576 | 8,905 | 6,743 | 6,708 | 2,197 | 0 |
| tokio-6752 | 15,996 | 13,274 | 13,211 | 2,785 | 0 |

No site moved between the two binding tiers. Every site that the prototype removed is removed by
the tip (0 kept, in each tree). The removed sites by the role that the tip stores, measured:

| Role at the tip (rule) | this repository | ripgrep | tokio |
|---|---:|---:|---:|
| `binding` or `local` (R1, and the signature parameters) | 3,342 | 541 | 668 |
| `attribute` (R2 member names, R4 type-path tails) | 1,927 | 1,556 | 1,856 |
| `declaration` (R5) | 73 | 35 | 72 |
| `reference`, no method candidate left (R3) | 43 | 65 | 189 |

What the tip removes beyond the prototype, measured and read in the source: R5 (70, 35 and 62
sites: `mod` names, enum variant names, trait method signatures, block-level `const` items, and
associated type names such as `type Captures = RegexCaptures;`), and 1 parameter of a
`function_signature_item` in tokio (`unsafe fn from_raw(ptr: ...)`). A seeded sample of 15 of these
sites per tree (seed 20261104): 45 of 45 are declarations or that parameter, and none is a use of
a definition. The parameter of a signature in this repository (`fn string(text: &mut String,
string: &CodePointString)`) the prototype already removed through R3.

The name-only tiers and the file graph (all Rust sites, measured; `is_reference` counts the sites
that the file graph of `map`, `communities` and `diagram` reads):

| Tree | unique (base / tip) | ambiguous (base / tip) | is_reference (base / tip) |
|---|---|---|---|
| this repository | 19,790 / 2,020 | 77,521 / 2,552 | 231,894 / 97,053 |
| ripgrep | 8,014 / 562 | 10,741 / 502 | 54,401 / 25,155 |
| tokio | 20,882 / 7,353 | 33,215 / 5,917 | 184,787 / 105,544 |

## The retrieval tier (D6 step 2)

The harness ran each frozen binary with `loc-bench run --agentless-bin`, seeded and no-seed, 150
instances per arm (measured; 23 to 24 s per arm). After each run, 0 files of the harness were newer
than the run's stamp.

- The base reproduces the stored arm of the last ranking change: 0 of 150 instances differ from the
  gf2 arms in `ok`, `ranked_files`, `resolved_seeds` and `unresolved_seeds`, in each arm.
- The tip against the base: `ranked_files` differ in 3 of 150 seeded instances and 7 of 150 no-seed
  instances. Each one is among the 10 instances whose checkout holds Rust files: seeded
  fapolicy-analyzer-809, meson-11366, ultralytics-17728; no-seed the same 3 and meson-13881,
  meson-14013, ultralytics-17499, ultralytics-17544. 0 lists outside the 10 changed.
- 7 of these 10 lists equal the prototype's lists. meson-11366 (both arms) swaps two C headers at
  ranks 17 and 18 whose ranks print equal (0.003853), and fapolicy-analyzer-809 (no-seed) differs
  from the prototype's list at ranks 18 and 19. Derived cause: R5, which the prototype did not have
  (9 declaration sites in the Rust test files of the meson checkout, 303 in fapolicy-analyzer).
- recall@10 is equal in 150 of 150 instances of each arm. One precision metric moves: no-seed
  ultralytics-17499 holds its gold file at rank 14, not 13. The paired bootstrap (10,000 samples,
  seed 20261104): seeded, every delta is 0 with a 95% interval of [0, 0]; no-seed, recall@10 0
  [0, 0], MAP -0.000037 [-0.000110, 0] (0 instances better, 1 worse), nDCG@10 0, acc_any@5 0.

## The surface diff (D6 step 3)

`surface_dump.py` of `v0.11.0rc1` (dump mode) and `scripts/surface_cmp.py`, base against tip, 8
workers, a 120 s timeout, on copies of the checkouts. The Loc-Bench targets are the 150 of the P2
surface diff (sampled once by the Python build; 1,206 queries). The 3 Rust trees get 124 targets,
the 41, 42 and 41 of the specification's refs preview: `refs --json` and `explain` on each, and 15
`map` and 15 `cycles` calls (278 queries). Both builds listed identical `find-symbol` frames, and no
call timed out or changed its exit code. The same answers carry `truncated` in both builds: 18
(Loc-Bench) and 6 (Rust trees).

| Surface | Loc-Bench changed / calls | Rust trees changed / calls |
|---|---|---|
| cycles | 0 / 150 | 0 / 15 |
| explain | 1 / 228 | 99 / 124 |
| map | 3 / 150 | 15 / 15 |
| refs | 2 / 678 | 121 / 124 |

- Loc-Bench: the 6 changed answers are in fapolicy-analyzer-809 (`map`, `refs` and `explain` of
  `user`, `refs` of `User.from_str`), meson-11366 (`map`, rank values only) and ultralytics-17728
  (`map`). The 140 targets without a Rust file: 0 changed answers.
- refs rows, measured: 392 rows move (386 in the Rust trees, 6 in Loc-Bench). From a binding tier
  to name-only-ambiguous 195, unique to name-only-ambiguous 41, and out of the answer 141; 15 enter
  one answer. Each moved row is classified by the role that the tip stores for the name on that line
  (derived): R5 declaration 141, R2 or R4 member or type-path tail 133, R1 parameter or local 74, R3
  bare name that lost its method candidates 29, and 15 rows of `refs Sender.drop`, which is
  truncated in both builds, so the token ceiling keeps other groups. No row gains a binding tier,
  and no row moves up from name-only-ambiguous.
- `total` falls in 26 of 124 Rust-tree refs answers and in 2 of 678 Loc-Bench refs answers. In each
  untruncated answer, the rows that leave are exactly the lines where the tip holds the name only
  as a declaration (R5). The largest: 98 lines `const LONG: &str = long!(` in function bodies of
  ripgrep's crates/core/app.rs, which bound at the unique tier to the bench constant `LONG`.
- explain, measured: 0 fan-in or fan-out rows gain a binding tier (352 answers). In the Rust trees,
  the fan-in falls for 26 of the 27 method targets and rises for none. 63 rows that appear at name-only-ambiguous were hidden by
  the base's display cap ("... N more edges at this tier not listed"); 5 rows move to the unique
  tier because R3 removed their method candidates.
- The loss count of `surface_cmp.py` (answers that shrank): 3 (Loc-Bench) and 123 (Rust trees).
  Each loss is a removed binding (R1 to R4) or a declaration line (R5), by the classification above.
  Some removed bindings were true calls: of the member rows that leave a binding tier in the refs
  answers on method targets, 18 of 97 (Rust trees) and 2 of 2 (Loc-Bench, `User::from_str(..)`)
  call through `self` with the same impl owner, or through the owner type (derived from the
  receiver text). They bound only through `mod tests { use super::*; }`. Binding by owner is the
  separate change R6.
- A seeded sample of the moved rows (31: 25 in the Rust trees, 6 in Loc-Bench, seed 20261104),
  read in the source: 31 of 31 are in the class that the classification gives.
- `health --json` on the 3 Rust trees (not in D6): hubs 5,226 -> 5,145, 1,725 -> 1,629 and 2,953
  -> 2,761. Orphan candidates 2 -> 3 (`ordered_map`, which only a string in
  `#[serde(serialize_with = "ordered_map")]` names), 7 -> 7 and 20 -> 21; unused exports 2 -> 2,
  11 -> 11 and 50 -> 51.

## callee_overlap on the CO1B reading set (D6 step 4)

`lint --repo <tree> --since <root or merge-base> --only callee_overlap --json`, as CO1B ran it, with
both builds, on copies of the 4 whole-tree trees (this repository at c2daf41, ripgrep-2576,
tokio-6752, matplotlib-13989) and on worktrees of the 10 pull requests whose rows the reading set
holds (#20, #24, #26, #30, #50, #51, #58, #59 in Python; #64, #66 in Rust). 28 runs, every exit 0.

- The base rows equal CO1B's rows in all 14 trees.
- Python (8 pull requests and matplotlib): base and tip JSON are byte-equal.
- Rust: this repository 32 -> 24 rows (11 go, 1 loses a callee, 3 are new), ripgrep 11 -> 2,
  tokio 10 -> 1, #64 1 -> 0, #66 2 -> 2. The tip's findings equal the prototype's findings field by
  field in all 5 Rust trees, so R5 and the signature parameters change no row.
- The 4 named false edges: szri, nxkq and vwfd give no row; u859 keeps its row without the callee
  `record`.
- The 33 Rust rows that both readers labeled: false_edge 16 (15 go, u859 stays), duplicate 5 (3
  go: yza9, jn7h, nz7h), sibling 12 (3 go: pz5e, nz7r, vizr). The 6 lost true rows shared callees
  that bound only through the self-glob.
- 3 rows are new on this repository: `validate ~ patch_step` (crates/cli/src/commands/write.rs),
  `assignment ~ variable` (crates/facts/src/extract/lua_assignments.rs) and `text_or_null ~
  int_or_null` (crates/hook/src/report/field.rs). Their blind packets, in the CO1B format, are
  `/var/tmp/claude-rust-port/rsm/packets` (zmgw, zeck, gcsd; the blind check finds 0 leaks).
  One blind reader labeled all 3 `sibling`, with high confidence and no false edge (work record
  `/var/tmp/claude-rust-port/rsm-r1/labels.jsonl`, 2026-11-04): two handlers of one dispatch, two
  members of a subcommand-parser builder family, and two members of a typed field-reader family.

## The suites (D6 step 1) and the exposure check (D6 step 5)

The suites ran on a4207bd in ticket RSB (`/var/tmp/claude-rust-port/rsb/status.md`): every command
of AGENTS.md exit 0 (1,597 tests), `scripts/gate-selftest.sh` 20 checks and 159 violations, and
every fixture and golden diff as predicted. The planted callee_overlap goldens did not change.

The exposure check is the specification's: on the 2026-09-27 hooked arm, `find_referencing_symbols`
ran 10 times in 9 instances and `symbols(explain)` 5 times in 5, and no graph operation ran. An
agentic run cannot see this change, so no paid run was bought.

## Where the predictions missed

- Binding-tier sites: predicted 38,896, 6,720 and 13,213 less the signature parameters; measured
  38,897, 6,708 and 13,211. The prototype's classifier labels the value of a `const` item
  (`const MAX_JOBS: usize = MAX_CANDIDATES;`) as a declaration name: 1 such site here and 2 in
  tokio, which are true references and keep binding. R5 also marks associated type names, which the
  classifier labels as types: 12 in ripgrep, 3 in tokio.
- The retrieval tier: predicted 2 seeded and 6 no-seed changed lists, equal to the prototype's;
  measured 3 and 7, and 2 lists that differ from the prototype's (R5, above).
- The surface diff: D6 predicted `total` equal in every refs answer. It falls by the R5 lines, as D3
  and Q5 of the specification name. No refs row moved up to unique, which D6 allowed.
