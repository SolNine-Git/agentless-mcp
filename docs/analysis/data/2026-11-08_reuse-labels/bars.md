# DUP3 bars for `reuse_candidates`, fixed before any label is read

Fixed: 2026-11-08 18:56 EST. No label of this round exists yet. These bars and reading rules do not change after
this point.

Build: `/var/tmp/claude-rust-port/plandoc/clone`, branch `feat/trace-tool`, commit
`ca23423f0d8bd2c88f61a44dd330ae94f7969a48`. Release binary sha256
`cc9d6a71de0ecf648cc10a784c7da04796417e320eac95456ddffce7637941d4`, frozen at
`/var/tmp/claude-rust-port/dup3/bin/ca23423/agentless-mcp`. Check settings as built
(`crates/checks/src/reuse.rs:33-38`): floor 0.3, 3 or more action names, a name counts at 5% or less of functions,
at most 2 rows per function.

Held-out repositories: `tokio-rs__tokio-6752` (Rust) and `sympy__sympy-24661` (Python), copies of the frozen bench
copies.

## The bars (design section 7.4, owner-accepted 2026-11-08; brief DUP3)

- **Bar C (card):** precision of 30% or more over 40 or more card rows, Wilson lower bound 15% or more, and at most
  1 card in 4 with a row.
- **Bar A (hook advisory):** at most 40% false positives over 40 or more rows.

## Reading rules, fixed with the bars

1. **Card row.** A row that `lint --since <base> --only reuse_candidates` gives a sampled function F as the added
   function, at most 2 per F. The base is a child commit of HEAD whose copy of F's file lacks F's symbol span (its
   first line, at an attribute or decorator, to its last line). The working tree is HEAD.
2. **Card sample.** The frame is every function that the check reads (metric scope) with 3 or more action names and
   no test or protocol mark: the check's own `kept` set. The frame is shuffled with seed 20261108, and functions are
   taken in that order until 40 or more functions are drawn and 40 or more card rows are held, per repository.
3. **Share of cards with a row.** Sampled functions with 1 or more rows over sampled functions. "At most 1 card in
   4" means this share is 25% or less. The share over every function in metric scope is also reported, derived
   (a function outside the frame never gets a row).
4. **Labels.** Each pair has one owner batch, and its label there is the label that counts. The second label of a
   double-labelled pair counts only toward agreement. Precision is `duplicate` over labelled rows. A false positive
   is a row labelled `related` or `unrelated`. A low-confidence label counts as given. A missing label is reported
   and left out of the denominator.
5. **Interval.** Wilson score, 95% (z = 1.96).
6. **Verdict.** Each bar is read on the card rows of both repositories together (pooled). The per-repository figures
   are reported beside it, and a bar that passes pooled but fails in one repository is reported as such. Bar C
   passes only when all three conditions hold. Bar A reads the same card rows: false positives 40% or less, and 40
   or more rows.
7. **Secondary reads (reported, never verdicts).** Precision of cross-file against same-file card rows (the
   design's cross-file rule), and the bar C and bar A arithmetic on the cross-file card rows only. The first-ranked
   row of each card. Precision of the global top 45 cross-file pairs per repository.
8. **Controls.** 8 per batch from the 2026-11-02 labels, 4 `duplicate` and 4 `related`, with their known labels.
   Their agreement with the known labels is reported. No rate is corrected by them.
