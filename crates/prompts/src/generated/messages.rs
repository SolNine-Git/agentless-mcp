//! The message templates.
//! `cargo xtask generate` writes this file from the prompt JSON files.

use crate::template::{Count, Piece, Template, Text};

/// A text that takes no values.
pub const CACHE_ABSENT_REFRESHING: &str = "a background refresh is in progress";

/// A template that takes `repo_root`.
pub const CACHE_BUILD_HINT: Template<Text> = Template::new(&[
    Piece::Text(
        "tag cache is empty: parsing is on demand; run agentless-mcp \
        index --repo ",
    ),
    Piece::Value(0),
    Piece::Text(" to build it"),
]);

/// A text that takes no values.
pub const CACHE_DISCARDED_NO_INDEX: &str = "cache discarded: no index has been built";

/// A template that takes (`found`, `expected`).
pub const CACHE_DISCARDED_OLD_SCHEMA: Template<(Text, Count)> = Template::new(&[
    Piece::Text("cache discarded: schema "),
    Piece::Value(0),
    Piece::Text(" predates "),
    Piece::Value(1),
    Piece::Text(", rerun agentless-mcp index"),
]);

/// A template that takes `repo_root`.
pub const CACHE_DISCARDED_OTHER_REPO: Template<Text> = Template::new(&[
    Piece::Text("cache discarded: it was built for "),
    Piece::Value(0),
]);

/// A text that takes no values.
pub const CACHE_STALE_REFRESHING: &str = "changed files parse live; a background refresh is in \
        progress";

/// A template that takes `repo_root`.
pub const CACHE_STALE_REMEDIATION: Template<Text> = Template::new(&[
    Piece::Text("changed files parse live; run agentless-mcp index --repo "),
    Piece::Value(0),
    Piece::Text(" for performance"),
]);

/// A template that takes (`count`, `listed`).
pub const CLONE_AND_MORE: Template<(Count, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" and "),
    Piece::Value(0),
    Piece::Text(" more"),
]);

/// A template that takes (`path`, `line`).
pub const CLONE_DOES_NOT_PARSE: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" does not parse: the first syntax error is at line "),
    Piece::Value(1),
    Piece::Text(
        ". Clone blocks are indexed only in a file that parses. Fix the \
        syntax error, then index the file again.",
    ),
]);

/// A template that takes (`paths`, `count`, `total`).
pub const CLONE_INDEX_GAPS: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("not checked: clone_delta did not index "),
    Piece::Value(1),
    Piece::Text(" of "),
    Piece::Value(2),
    Piece::Text(
        " repository files, so it does not see a copy of a block in \
        them: ",
    ),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`path`, `reason`).
pub const CLONE_INDEX_GAPS_EVIDENCE: Template<(Text, Text)> = Template::new(&[
    Piece::Text("first: "),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const CLONE_INDEX_STOPPED: &str = "stopped: the clone index reached its time budget before \
        this file";

/// A template that takes `reason`.
pub const CLONE_INDEX_UNLISTED: Template<Text> = Template::new(&[
    Piece::Text("not checked: clone_delta could not list the repository files ("),
    Piece::Value(0),
    Piece::Text("), so it compared the changed files only with each other."),
]);

/// A template that takes (`path`, `language`, `languages`).
pub const CLONE_LANGUAGE_UNSUPPORTED: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": clone blocks have no rules for language '"),
    Piece::Value(1),
    Piece::Text("'. They are indexed only in these languages: "),
    Piece::Value(2),
    Piece::Text("."),
]);

/// A template that takes (`start`, `end`, `count`).
pub const CLONE_NEW_COPY: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("lines "),
    Piece::Value(0),
    Piece::Text("-"),
    Piece::Value(1),
    Piece::Text(
        " add a copy of a block, with names and literals ignored, and \
        this repository now holds ",
    ),
    Piece::Value(2),
    Piece::Text(
        " copies of it. Each copy must be changed with the others. Move \
        the shared code into one named function, and call it from each \
        place.",
    ),
]);

/// A template that takes `sites`.
pub const CLONE_NEW_COPY_EVIDENCE: Template<Text> =
    Template::new(&[Piece::Text("also at "), Piece::Value(0)]);

/// A template that takes (`path`, `reason`).
pub const CLONE_NEW_NOT_INDEXED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: clone_delta did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text after the change cannot be indexed. "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const CLONE_NO_TAG_CACHE: &str = "not checked: in the hook form, clone_delta reads the \
        repository's clone blocks from the tag cache only, and this run \
        has no tag cache. Run agentless-mcp index on the repository to \
        turn the check on.";

/// A text that takes no values.
pub const CLONE_NO_TAG_CACHE_EVIDENCE: &str = "no tag cache";

/// A template that takes (`path`, `reason`).
pub const CLONE_OLD_NOT_INDEXED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: clone_delta did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text before the change cannot be indexed. "),
    Piece::Value(1),
]);

/// A template that takes (`path`, `detail`).
pub const CLONE_TOKENIZER_REJECTS: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": Python's tokenizer rejects this file ("),
    Piece::Value(1),
    Piece::Text(
        "). Clone blocks are indexed only in a file the tokenizer \
        accepts, the same as scb-check. Fix the line the error names, \
        then index the file again.",
    ),
]);

/// A text that takes no values.
pub const COMMUNITIES_NOT_CONVERGED: &str = "note: community detection stopped at its pass limit \
        before the partition settled, so these communities are a \
        partial answer";

/// A template that takes (`qualname`, `old_cc`, `new_cc`, `old_sloc`, `new_sloc`, `threshold`).
pub const COMPLEXITY_CROSSED: Template<(Text, Count, Count, Count, Count, Count)> =
    Template::new(&[
        Piece::Value(0),
        Piece::Text(": complexity "),
        Piece::Value(1),
        Piece::Text(" -> "),
        Piece::Value(2),
        Piece::Text(", "),
        Piece::Value(3),
        Piece::Text(" -> "),
        Piece::Value(4),
        Piece::Text(
            " source lines. This change takes the function over the \
        threshold (",
        ),
        Piece::Value(5),
        Piece::Text(
            "). Put the new branches in a new named function and call it, or \
        refactor this function first in its own change.",
        ),
    ]);

/// A template that takes (`path`, `line`).
pub const COMPLEXITY_DOES_NOT_PARSE: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" does not parse: the first syntax error is at line "),
    Piece::Value(1),
    Piece::Text(
        ". Complexity is measured only for a file that parses. Fix the \
        syntax error, then measure the file again.",
    ),
]);

/// A template that takes (`old_cc`, `new_cc`, `old_sloc`, `new_sloc`, `threshold`).
pub const COMPLEXITY_EVIDENCE: Template<(Count, Count, Count, Count, Count)> = Template::new(&[
    Piece::Text("cc "),
    Piece::Value(0),
    Piece::Text(" -> "),
    Piece::Value(1),
    Piece::Text("; sloc "),
    Piece::Value(2),
    Piece::Text(" -> "),
    Piece::Value(3),
    Piece::Text("; threshold "),
    Piece::Value(4),
]);

/// A template that takes (`qualname`, `old_cc`, `new_cc`, `old_sloc`, `new_sloc`, `threshold`).
pub const COMPLEXITY_GREW: Template<(Text, Count, Count, Count, Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": complexity "),
    Piece::Value(1),
    Piece::Text(" -> "),
    Piece::Value(2),
    Piece::Text(", "),
    Piece::Value(3),
    Piece::Text(" -> "),
    Piece::Value(4),
    Piece::Text(" source lines. This function is already above the threshold ("),
    Piece::Value(5),
    Piece::Text(
        "), and this change adds more branching to it. Put the new \
        branches in a new named function and call it, or refactor this \
        function first in its own change.",
    ),
]);

/// A template that takes (`path`, `language`, `languages`).
pub const COMPLEXITY_LANGUAGE_UNSUPPORTED: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": complexity has no rules for language '"),
    Piece::Value(1),
    Piece::Text("'. It measures only these languages: "),
    Piece::Value(2),
    Piece::Text("."),
]);

/// A template that takes (`path`, `reason`).
pub const COMPLEXITY_NEW_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: complexity_delta did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text after the change cannot be measured. "),
    Piece::Value(1),
]);

/// A template that takes (`qualname`, `new_cc`, `new_sloc`, `threshold`).
pub const COMPLEXITY_NEW_UNIT: Template<(Text, Count, Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": complexity "),
    Piece::Value(1),
    Piece::Text(", "),
    Piece::Value(2),
    Piece::Text(
        " source lines. This function is new, or moved from another \
        file, and it is above the threshold (",
    ),
    Piece::Value(3),
    Piece::Text(
        "). Split it into smaller named functions. If it moved here \
        unchanged, it was already above the threshold in its old file.",
    ),
]);

/// A template that takes (`new_cc`, `new_sloc`, `threshold`).
pub const COMPLEXITY_NEW_UNIT_EVIDENCE: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("cc "),
    Piece::Value(0),
    Piece::Text("; sloc "),
    Piece::Value(1),
    Piece::Text("; threshold "),
    Piece::Value(2),
]);

/// A template that takes (`path`, `reason`).
pub const COMPLEXITY_OLD_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: complexity_delta did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text before the change cannot be measured. "),
    Piece::Value(1),
]);

/// A template that takes (`path`, `detail`).
pub const COMPLEXITY_TOKENIZER_REJECTS: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": Python's tokenizer rejects this file ("),
    Piece::Value(1),
    Piece::Text(
        "). Complexity is measured only for a file the tokenizer \
        accepts, the same as scb-check. Fix the line the error names, \
        then measure the file again.",
    ),
]);

/// A template that takes (`config`, `count`, `chars`).
pub const CONFIG_BOUNDARY_GLOBS_DROPPED: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("boundary_globs in "),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(" entries are not repo-relative globs of at most "),
    Piece::Value(2),
    Piece::Text(" characters. They are ignored."),
]);

/// A template that takes (`config`, `limit`, `chars`).
pub const CONFIG_BOUNDARY_GLOBS_NOT_LIST: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("boundary_globs in "),
    Piece::Value(0),
    Piece::Text(" must be a list of at most "),
    Piece::Value(1),
    Piece::Text(" repo-relative globs, each at most "),
    Piece::Value(2),
    Piece::Text(" characters. The key is ignored."),
]);

/// A template that takes (`config`, `count`, `limit`).
pub const CONFIG_BOUNDARY_GLOBS_TOO_MANY: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("boundary_globs in "),
    Piece::Value(0),
    Piece::Text(" holds "),
    Piece::Value(1),
    Piece::Text(" globs, over the "),
    Piece::Value(2),
    Piece::Text(" cap. The key is ignored."),
]);

/// A template that takes (`config`, `check`, `modes`).
pub const CONFIG_EROSION_GATE_MODE: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Text("erosion_gate."),
    Piece::Value(1),
    Piece::Text(" in "),
    Piece::Value(0),
    Piece::Text(" must be one of "),
    Piece::Value(2),
    Piece::Text(". The entry is ignored."),
]);

/// A template that takes (`config`, `modes`).
pub const CONFIG_EROSION_GATE_NOT_OBJECT: Template<(Text, Text)> = Template::new(&[
    Piece::Text("erosion_gate in "),
    Piece::Value(0),
    Piece::Text(" must be an object that maps a check name to one of "),
    Piece::Value(1),
    Piece::Text(". The key is ignored."),
]);

/// A template that takes (`config`, `checks`, `count`).
pub const CONFIG_EROSION_GATE_UNKNOWN: Template<(Text, Text, Count)> = Template::new(&[
    Piece::Text("erosion_gate in "),
    Piece::Value(0),
    Piece::Text(" holds "),
    Piece::Value(2),
    Piece::Text(" entries that name no gated check ("),
    Piece::Value(1),
    Piece::Text("). They are ignored."),
]);

/// A template that takes (`rule`, `unit`, `capture`).
pub const CONTRACT_EVIDENCE: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Text("rule "),
    Piece::Value(0),
    Piece::Text("; unit "),
    Piece::Value(1),
    Piece::Text("; capture "),
    Piece::Value(2),
]);

/// A template that takes (`rule`, `unit`, `capture`, `why`).
pub const CONTRACT_NEW_HIT: Template<(Text, Text, Text, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(": "),
    Piece::Value(2),
    Piece::Text(" is new in a boundary module ("),
    Piece::Value(0),
    Piece::Text("). "),
    Piece::Value(3),
]);

/// A template that takes (`path`, `reason`).
pub const CONTRACT_NEW_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: contract_drift did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text after the change cannot be measured. "),
    Piece::Value(1),
]);

/// A template that takes `config`.
pub const CONTRACT_NO_BOUNDARY: Template<Text> = Template::new(&[
    Piece::Text(
        "not checked: contract_drift has no boundary modules to check. \
        Set boundary_globs in ",
    ),
    Piece::Value(0),
    Piece::Text(" to the globs of the modules that parse foreign data."),
]);

/// A text that takes no values.
pub const CONTRACT_NO_BOUNDARY_EVIDENCE: &str = "boundary_globs is empty";

/// A template that takes (`path`, `language`).
pub const CONTRACT_NO_RULES: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: contract_drift has no rules for "),
    Piece::Value(1),
    Piece::Text(", so it did not check "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`path`, `reason`).
pub const CONTRACT_OLD_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: contract_drift did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text before the change cannot be measured. "),
    Piece::Value(1),
]);

/// A template that takes (`shortened`, `total`, `budget`).
pub const EXPAND_BATCH_SHORTENED: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("note: "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" bodies were shortened to fit the "),
    Piece::Value(2),
    Piece::Text(
        "-token expansion budget; every requested id still appears, each \
        marked where it was cut. Expand fewer ids per call for whole \
        bodies.",
    ),
]);

/// A template that takes (`shown`, `total`).
pub const EXPAND_BODY_TRUNCATED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(
        " lines shown: the batch did not fit the output ceiling. Expand \
        this id on its own for the whole body.",
    ),
]);

/// A template that takes (`requested`, `seats`).
pub const EXPAND_NO_ROOM: Template<(Count, Count)> = Template::new(&[
    Piece::Text("not expanded: "),
    Piece::Value(0),
    Piece::Text(" ids exceed the "),
    Piece::Value(1),
    Piece::Text(" one call can answer; retry with at most that many."),
]);

/// A template that takes (`target`, `count`).
pub const EXPLAIN_TIE: Template<(Text, Count)> = Template::new(&[
    Piece::Text("ambiguous: "),
    Piece::Value(1),
    Piece::Text(" definitions match "),
    Piece::Value(0),
    Piece::Text(
        " equally; this explains the first in path order. Pass a stable \
        id to explain another.",
    ),
]);

/// A template that takes `query`.
pub const FIND_NO_MATCHES: Template<Text> = Template::new(&[
    Piece::Text("no matching symbols: no declared symbol has '"),
    Piece::Value(0),
    Piece::Text(
        "' in its name or qualified name, matched case-insensitively. A \
        parameter, a local variable, an attribute or a string literal \
        is not a declared symbol; grep for the literal to find those.",
    ),
]);

/// A template that takes (`kind`, `query`, `kinds`).
pub const FIND_NO_MATCHES_KIND: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Text("no matching symbols of kind '"),
    Piece::Value(0),
    Piece::Text("': '"),
    Piece::Value(1),
    Piece::Text("' does match symbols of other kinds: "),
    Piece::Value(2),
    Piece::Text(". Retry with one of those kinds, or with no kind."),
]);

/// A template that takes (`query`, `listed`).
pub const FIND_UNPARSED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("note: "),
    Piece::Value(0),
    Piece::Text(" is spelled where a file did not parse ("),
    Piece::Value(1),
    Piece::Text("), so a declaration there may be missing from this answer"),
]);

/// A template that takes `count`.
pub const GROUPED_IDS: Template<Count> =
    Template::new(&[Piece::Text("("), Piece::Value(0), Piece::Text(" ids)")]);

/// A template that takes (`sha`, `shown`, `total`).
pub const HISTORY_BODY_TRUNCATED: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(1),
    Piece::Text(" of "),
    Piece::Value(2),
    Piece::Text(" body lines shown for "),
    Piece::Value(0),
    Piece::Text("; git show -s --format=%B "),
    Piece::Value(0),
    Piece::Text(" prints the whole message"),
]);

/// A template that takes (`path`, `start`, `end`).
pub const HISTORY_DIRTY_FILE: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("note: "),
    Piece::Value(0),
    Piece::Text(
        " has uncommitted changes; the span was located in the committed \
        copy of the file (lines ",
    ),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(" there), so these commits follow the committed definition"),
]);

/// A template that takes (`path`, `start`, `end`, `note`).
pub const HISTORY_DIRTY_UNKNOWN: Template<(Text, Count, Count, Text)> = Template::new(&[
    Piece::Text("note: git could not say whether "),
    Piece::Value(0),
    Piece::Text(" has uncommitted changes ("),
    Piece::Value(3),
    Piece::Text(
        "); the span was located in the committed copy of the file \
        (lines ",
    ),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(" there)"),
]);

/// A template that takes `note`.
pub const HISTORY_GIT_FAILED: Template<Text> =
    Template::new(&[Piece::Text("git could not answer: "), Piece::Value(0)]);

/// A template that takes `detail`.
pub const HISTORY_GIT_OUTPUT_MALFORMED: Template<Text> = Template::new(&[
    Piece::Text("git printed log output history cannot read: "),
    Piece::Value(0),
    Piece::Text(
        ". history refuses a partial read rather than answer from one. \
        Check that this git honours --no-patch together with -L, and \
        that no filter rewrites git log output.",
    ),
]);

/// A template that takes `shown`.
pub const HISTORY_MORE_COMMITS: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(
        " newest commits shown; older commits touch this span (raise \
        limit for the rest)",
    ),
]);

/// A template that takes (`path`, `start`, `end`).
pub const HISTORY_NO_COMMITS: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("no commit touches lines "),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(" of "),
    Piece::Value(0),
]);

/// A template that takes `note`.
pub const HISTORY_NO_GIT: Template<Text> =
    Template::new(&[Piece::Text("history needs git: "), Piece::Value(0)]);

/// A template that takes (`count`, `bytes`).
pub const HISTORY_OUTPUT_CAPPED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... git output was cut at "),
    Piece::Value(1),
    Piece::Text(" bytes; the "),
    Piece::Value(0),
    Piece::Text(" complete commits above are shown"),
]);

/// A template that takes (`path`, `start`, `end`, `bytes`).
pub const HISTORY_OUTPUT_CAPPED_NO_COMMITS: Template<(Text, Count, Count, Count)> =
    Template::new(&[
        Piece::Text("unsatisfiable: git output reached the "),
        Piece::Value(3),
        Piece::Text(
            " byte cap before the first complete commit. The newest commit \
        on lines ",
        ),
        Piece::Value(1),
        Piece::Text("-"),
        Piece::Value(2),
        Piece::Text(" of "),
        Piece::Value(0),
        Piece::Text(" carries a message larger than the cap; run git log -L"),
        Piece::Value(1),
        Piece::Text(","),
        Piece::Value(2),
        Piece::Text(":"),
        Piece::Value(0),
        Piece::Text(" to read it."),
    ]);

/// A template that takes `path`.
pub const HISTORY_PATH_NOT_IN_HEAD: Template<Text> = Template::new(&[
    Piece::Text("unsatisfiable: "),
    Piece::Value(0),
    Piece::Text(
        " is not in HEAD (untracked, ignored, or only staged). Commit \
        it, or ask about a tracked file.",
    ),
]);

/// A template that takes (`path`, `start`, `end`, `shown`).
pub const HISTORY_SEATS_CAPPED: Template<(Text, Count, Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(3),
    Piece::Text(
        " commits shown, the most one answer seats; older commits touch \
        this span (ask about a narrower span, or run git log -L",
    ),
    Piece::Value(1),
    Piece::Text(","),
    Piece::Value(2),
    Piece::Text(":"),
    Piece::Value(0),
    Piece::Text(" for the rest)"),
]);

/// A text that takes no values.
pub const HISTORY_SHALLOW: &str = "note: this clone is shallow, so the oldest commit listed may \
        stand in for older history";

/// A template that takes `note`.
pub const HISTORY_SHALLOW_UNKNOWN: Template<Text> = Template::new(&[
    Piece::Text("note: git could not say whether this clone is shallow ("),
    Piece::Value(0),
    Piece::Text(
        "); in a shallow clone the oldest commit listed may stand in for \
        older history",
    ),
]);

/// A template that takes `name`.
pub const HISTORY_SIGNATURE_CHANGED: Template<Text> = Template::new(&[
    Piece::Text("note: "),
    Piece::Value(0),
    Piece::Text(
        " has a different signature in the committed copy than in the \
        working tree; these commits follow the committed definition",
    ),
]);

/// A template that takes (`path`, `start`, `end`).
pub const HISTORY_SPAN_BEYOND_HEAD: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("unsatisfiable: lines "),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(" of "),
    Piece::Value(0),
    Piece::Text(
        " are past the end of HEAD's copy. The working tree grew the \
        file since the last commit; commit, then ask again.",
    ),
]);

/// A template that takes (`path`, `name`).
pub const HISTORY_SYMBOL_NOT_IN_HEAD: Template<(Text, Text)> = Template::new(&[
    Piece::Text("unsatisfiable: "),
    Piece::Value(1),
    Piece::Text(" is not defined in the committed copy of "),
    Piece::Value(0),
    Piece::Text(
        ": it is new or renamed in the working tree, so no commit has \
        touched it yet",
    ),
]);

/// A template that takes (`target`, `reason`).
pub const HISTORY_TARGET_UNRESOLVED: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(
        ". Pass a stable id from a map, overview, find, explain or \
        locate answer; history is span-scoped, so a bare name is \
        refused.",
    ),
]);

/// A template that takes `checks`.
pub const LINT_NO_CHECKS: Template<Text> = Template::new(&[
    Piece::Text("no check selected. Name at least one of these checks: "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A text that takes no values.
pub const LINT_PATH_NEEDS_AFTER: &str = "--path and --after go together: --path names the file in \
        --repo, and --after names a file that holds its proposed new \
        text.";

/// A template that takes (`pair_checks`, `checks`).
pub const LINT_PATH_TAKES_PAIR_CHECKS: Template<(Text, Text)> = Template::new(&[
    Piece::Text("--path runs only the pair checks ("),
    Piece::Value(0),
    Piece::Text("), because it supplies whole-file text and no patch. Remove "),
    Piece::Value(1),
    Piece::Text(" from --only, or lint a patch with --candidates or --diff."),
]);

/// A template that takes `path`.
pub const LINT_REVISION_BINARY: Template<Text> = Template::new(&[
    Piece::Text("not checked: "),
    Piece::Value(0),
    Piece::Text(" is binary, so no pair check compared this file."),
]);

/// A template that takes `bytes`.
pub const LINT_REVISION_BINARY_EVIDENCE: Template<Count> = Template::new(&[
    Piece::Text("a NUL byte in the first "),
    Piece::Value(0),
    Piece::Text(" bytes"),
]);

/// A template that takes (`command`, `bytes`).
pub const LINT_REVISION_LISTING_CAPPED: Template<(Text, Count)> = Template::new(&[
    Piece::Text("not checked: git "),
    Piece::Value(0),
    Piece::Text(" printed more than "),
    Piece::Value(1),
    Piece::Text(
        " bytes, so the list of changed files is not complete and no \
        pair check compared any file. Review a shorter range of commits.",
    ),
]);

/// A template that takes (`revision`, `note`).
pub const LINT_REVISION_NOT_RESOLVED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: git did not resolve --since "),
    Piece::Value(0),
    Piece::Text(" ("),
    Piece::Value(1),
    Piece::Text("), so no pair check compared any file."),
]);

/// A template that takes `detail`.
pub const LINT_REVISION_OUTPUT_MALFORMED: Template<Text> = Template::new(&[
    Piece::Text("git printed output that the review cannot read: "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`path`, `status`).
pub const LINT_REVISION_STATUS: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: git reports "),
    Piece::Value(0),
    Piece::Text(" with status "),
    Piece::Value(1),
    Piece::Text(
        ", for example an unresolved merge conflict, so no pair check \
        compared this file.",
    ),
]);

/// A template that takes `status`.
pub const LINT_REVISION_STATUS_EVIDENCE: Template<Text> =
    Template::new(&[Piece::Text("status "), Piece::Value(0)]);

/// A template that takes `path`.
pub const LINT_REVISION_SUBMODULE: Template<Text> = Template::new(&[
    Piece::Text("not checked: "),
    Piece::Value(0),
    Piece::Text(
        " is a submodule. The pair checks read only the files of this \
        repository.",
    ),
]);

/// A text that takes no values.
pub const LINT_REVISION_SUBMODULE_EVIDENCE: &str = "submodule (mode 160000)";

/// A template that takes `revision`.
pub const LINT_REVISION_UNCHANGED: Template<Text> = Template::new(&[
    Piece::Text("not checked: no file changed since "),
    Piece::Value(0),
    Piece::Text(", so there is nothing to compare."),
]);

/// A text that takes no values.
pub const LINT_REVISION_UNCHANGED_EVIDENCE: &str = "0 changed files";

/// A template that takes (`command`, `note`).
pub const LINT_REVISION_UNLISTED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: git "),
    Piece::Value(0),
    Piece::Text(" did not list the changed files ("),
    Piece::Value(1),
    Piece::Text("), so no pair check compared any file."),
]);

/// A template that takes (`revision`, `note`).
pub const LINT_REVISION_UNRESOLVED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("--since "),
    Piece::Value(0),
    Piece::Text(" does not name a commit in this repository ("),
    Piece::Value(1),
    Piece::Text(
        "). Name a commit, for example --since \"$(git merge-base HEAD \
        main)\".",
    ),
]);

/// A text that takes no values.
pub const LINT_SINCE_ALONE: &str = "--since takes no --after. --since compares each file changed \
        since REV with its text in the working tree, so it needs no \
        other input.";

/// A template that takes (`pair_checks`, `checks`).
pub const LINT_SINCE_TAKES_PAIR_CHECKS: Template<(Text, Text)> = Template::new(&[
    Piece::Text("--since runs only the pair checks ("),
    Piece::Value(0),
    Piece::Text(
        "), because the other checks compare a patch with the current \
        tree and would see two states mixed. Remove ",
    ),
    Piece::Value(1),
    Piece::Text(" from --only."),
]);

/// A template that takes (`name`, `checks`).
pub const LINT_UNKNOWN_CHECK: Template<(Text, Text)> = Template::new(&[
    Piece::Text("unknown check '"),
    Piece::Value(0),
    Piece::Text("'. The checks are: "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes (`name`, `severities`).
pub const LINT_UNKNOWN_SEVERITY: Template<(Text, Text)> = Template::new(&[
    Piece::Text("unknown severity '"),
    Piece::Value(0),
    Piece::Text("'. The severities are: "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes `count`.
pub const MAP_FILE_UNREACHED: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(
        " symbols in this file; the focus has no reference path to it, \
        so no budget shows them",
    ),
]);

/// A template that takes (`limit`, `minimum`, `maximum`).
pub const MAP_LIMIT_OUT_OF_RANGE: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("orient operation 'map' rejects limit="),
    Piece::Value(0),
    Piece::Text(": the map's file cap runs "),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(
        ", which is the same bound repo_map publishes as max_files. The \
        wider ceiling on this parameter belongs to the communities and \
        cycles listings that share it. Retry with a limit in range.",
    ),
]);

/// A template that takes `seeds`.
pub const MAP_UNRESOLVED_SEEDS: Template<Text> = Template::new(&[
    Piece::Text(
        "// note: these focus seeds matched no file, no symbol, and no \
        name spelled in five or fewer non-test files of this \
        repository, so the ranking below is not focused on them: ",
    ),
    Piece::Value(0),
    Piece::Text(". Check the spelling, or seed with a file path from the tree."),
]);

/// A template that takes (`tool`, `operation`, `stray`, `accepted`, `required`).
pub const OP_REJECTS_PARAMETERS: Template<(Text, Text, Text, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" operation '"),
    Piece::Value(1),
    Piece::Text("' does not accept: "),
    Piece::Value(2),
    Piece::Text(". This operation accepts: "),
    Piece::Value(3),
    Piece::Text("; required: "),
    Piece::Value(4),
    Piece::Text(
        ". Drop the stray parameters, or pick the operation that takes \
        them.",
    ),
]);

/// A template that takes (`tool`, `operation`, `missing`, `accepted`, `required`).
pub const OP_REQUIRES_PARAMETERS: Template<(Text, Text, Text, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" operation '"),
    Piece::Value(1),
    Piece::Text("' is missing: "),
    Piece::Value(2),
    Piece::Text(". This operation accepts: "),
    Piece::Value(3),
    Piece::Text("; required: "),
    Piece::Value(4),
    Piece::Text(". Retry naming the missing parameters."),
]);

/// A template that takes `pattern`.
pub const OVERVIEW_STABLE_IDS: Template<Text> = Template::new(&[
    Piece::Text("stable ids: "),
    Piece::Value(0),
    Piece::Text(" -- nested symbols qualify as Class.method"),
]);

/// A template that takes `cap`.
pub const PAIR_BLOB_OVER_CAP: Template<Count> = Template::new(&[
    Piece::Text("skipped: the committed text exceeds the per-file cap of "),
    Piece::Value(0),
    Piece::Text(" bytes"),
]);

/// A template that takes (`path`, `failed`).
pub const PAIR_EDITS_UNAPPLIED: Template<(Text, Count)> = Template::new(&[
    Piece::Text("not checked: "),
    Piece::Value(1),
    Piece::Text(" of the edits to "),
    Piece::Value(0),
    Piece::Text(" did not apply, so no pair check compared this file."),
]);

/// A template that takes (`path`, `reason`).
pub const PAIR_NEW_UNREAD: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: the proposed text for "),
    Piece::Value(0),
    Piece::Text(" was not read ("),
    Piece::Value(1),
    Piece::Text("), so no pair check compared this file."),
]);

/// A template that takes (`path`, `reason`).
pub const PAIR_OLD_UNREAD: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: the text of "),
    Piece::Value(0),
    Piece::Text(" before the change was not read ("),
    Piece::Value(1),
    Piece::Text("), so no pair check compared this file."),
]);

/// A template that takes (`path`, `reason`).
pub const PAIR_WORKTREE_UNREAD: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: the working-tree text of "),
    Piece::Value(0),
    Piece::Text(" was not read ("),
    Piece::Value(1),
    Piece::Text("), so no pair check compared this file."),
]);

/// A template that takes `tool`.
pub const PATH_NEEDS_ENDPOINTS: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        " operation 'path' needs both source and target: each is a \
        stable id, a qualified name, or a repository-relative file \
        path. Retry naming both ends.",
    ),
]);

/// A template that takes (`target`, `name`).
pub const REFS_TARGET_UNRESOLVED: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" resolves to no symbol; these are the references to the name "),
    Piece::Value(1),
    Piece::Text(", wherever it is defined"),
]);

/// A text that takes no values.
pub const REPO_REFUSED_NO_ROOTS: &str = "repository refused: this server was started with no \
        roots, so it serves no repositories. Roots come from --root DIR \
        flags and from --roots-from FILE, which lists one repository \
        path per line.";

/// A template that takes `roots`.
pub const REPO_REFUSED_NOT_ALLOWED: Template<Text> = Template::new(&[
    Piece::Text(
        "repository refused: the requested root is not one of this \
        server's roots. Retry with repo_root set to one of: ",
    ),
    Piece::Value(0),
]);

/// A template that takes `file`.
pub const ROOTS_FILE_HINT: Template<Text> = Template::new(&[
    Piece::Text(
        "To serve another repository, append its absolute path as a line \
        in ",
    ),
    Piece::Value(0),
    Piece::Text("; the file is re-read on the next call, no restart needed."),
]);

/// A template that takes (`file`, `error`).
pub const ROOTS_FILE_UNREADABLE: Template<(Text, Text)> = Template::new(&[
    Piece::Text("roots file "),
    Piece::Value(0),
    Piece::Text(" cannot be read: "),
    Piece::Value(1),
    Piece::Text(
        ". Refusing rather than serving the stale copy; restore the \
        file, or restart the server without --roots-from.",
    ),
]);

/// A template that takes (`count`, `listed`).
pub const SCAN_SKIPPED_FILES: Template<(Count, Text)> = Template::new(&[
    Piece::Text("// warning: "),
    Piece::Value(0),
    Piece::Text(" files were skipped and are absent from this answer: "),
    Piece::Value(1),
    Piece::Text(
        ". A symbol in a skipped file reads as missing here even though \
        it may exist in the repository.",
    ),
]);

/// A text that takes no values.
pub const SERVER_NO_ROOTS: &str = "no repositories are served: this server was started with no \
        roots. Roots come from --root DIR flags and from --roots-from \
        FILE, which lists one repository path per line. A root \
        advertised by the client can only select among the roots the \
        server is configured with; it cannot add one.";

/// A template that takes `roots`.
pub const SERVER_ROOT_REQUIRED: Template<Text> = Template::new(&[
    Piece::Text(
        "repo_root is required: this server holds several repositories \
        and will not guess between them. Retry with repo_root set to \
        one of: ",
    ),
    Piece::Value(0),
]);

/// A template that takes (`path`, `start`, `end`, `total`).
pub const SLICE_RANGE_BEYOND_FILE: Template<(Text, Count, Count, Count)> = Template::new(&[
    Piece::Text("unsatisfiable: line range "),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(" is beyond "),
    Piece::Value(0),
    Piece::Text(" ("),
    Piece::Value(3),
    Piece::Text(" lines)"),
]);

/// A template that takes (`path`, `start`, `end`).
pub const SLICE_RANGE_NOT_A_RANGE: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("unsatisfiable: line range "),
    Piece::Value(1),
    Piece::Text("-"),
    Piece::Value(2),
    Piece::Text(" is not a range in "),
    Piece::Value(0),
    Piece::Text(": a start of 1 or more, and an end at or after the start"),
]);

/// A template that takes `pattern`.
pub const STABLE_IDS_PATTERN: Template<Text> =
    Template::new(&[Piece::Text("stable ids: "), Piece::Value(0)]);

/// A template that takes `surface`.
pub const SURFACE_V1_DEPRECATED: Template<Text> = Template::new(&[
    Piece::Text("--surface "),
    Piece::Value(0),
    Piece::Text(
        " publishes the v1 tools. They are deprecated and are removed in \
        0.11.0. Move to the v2 tools: agentless-mcp guide --section \
        the-two-surfaces has the mapping.",
    ),
]);

/// A template that takes (`tool`, `operation`, `operations`).
pub const UNKNOWN_OPERATION: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" has no operation named '"),
    Piece::Value(1),
    Piece::Text("'. Valid operations are: "),
    Piece::Value(2),
    Piece::Text(". Retry with one of those."),
]);
