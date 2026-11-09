//! The message templates.
//! `cargo xtask generate` writes this file from the prompt JSON files.

use crate::template::{Count, Piece, Template, Text};

/// A text that takes no values.
pub const ACCEPTS_ANY_NUMBER: &str = "a number";

/// A text that takes no values.
pub const ACCEPTS_ARRAY: &str = "an array";

/// A text that takes no values.
pub const ACCEPTS_BOOL: &str = "true or false";

/// A template that takes `listed`.
pub const ACCEPTS_CHOICE: Template<Text> =
    Template::new(&[Piece::Text("one of "), Piece::Value(0)]);

/// A template that takes (`minimum`, `maximum`).
pub const ACCEPTS_INTEGER: Template<(Count, Count)> = Template::new(&[
    Piece::Text("an integer from "),
    Piece::Value(0),
    Piece::Text(" to "),
    Piece::Value(1),
]);

/// A template that takes `minimum`.
pub const ACCEPTS_INTEGER_FROM: Template<Count> = Template::new(&[
    Piece::Text("an integer of "),
    Piece::Value(0),
    Piece::Text(" or more"),
]);

/// A template that takes (`minimum`, `ceiling`).
pub const ACCEPTS_NUMBER: Template<(Count, Text)> = Template::new(&[
    Piece::Text("a number above "),
    Piece::Value(0),
    Piece::Text(" and at most "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const ACCEPTS_OBJECT: &str = "an object";

/// A text that takes no values.
pub const ACCEPTS_SCHEMA: &str = "a value that fits its schema in tools/list";

/// A text that takes no values.
pub const ACCEPTS_SERVER_NAME: &str = "a name of ASCII letters, digits, _ and -";

/// A text that takes no values.
pub const ACCEPTS_TEXT: &str = "a string";

/// A template that takes (`minimum`, `maximum`).
pub const ACCEPTS_TEXT_OR_TEXTS: Template<(Count, Count)> = Template::new(&[
    Piece::Text("a string or a list of "),
    Piece::Value(0),
    Piece::Text(" to "),
    Piece::Value(1),
    Piece::Text(" strings"),
]);

/// A text that takes no values.
pub const ACCEPTS_TEXTS: &str = "a list of strings";

/// A template that takes (`accepted`, `name`, `found`).
pub const ARGUMENT_INVALID: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" is "),
    Piece::Value(2),
    Piece::Text(", but must be "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`accepted`, `name`).
pub const ARGUMENT_MISSING: Template<(Text, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" is required and must be "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`tool`, `name`).
pub const ARGUMENT_UNKNOWN: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" has no parameter "),
    Piece::Value(1),
    Piece::Text("."),
]);

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

/// A text that takes no values.
pub const CALLEE_OVERLAP_COMPARED: &str = "no function";

/// A template that takes (`path`, `line`).
pub const CALLEE_OVERLAP_DOES_NOT_PARSE: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" does not parse: the first syntax error is at line "),
    Piece::Value(1),
    Piece::Text(
        ". Functions are compared only in a file that parses. Fix the \
        syntax error, then run the check again.",
    ),
]);

/// A template that takes (`target`, `name`, `count`, `total`).
pub const CALLEE_OVERLAP_FOUND: Template<(Text, Text, Count, Count)> = Template::new(&[
    Piece::Text("`"),
    Piece::Value(1),
    Piece::Text("` calls "),
    Piece::Value(2),
    Piece::Text(" of the "),
    Piece::Value(3),
    Piece::Text(" functions that `"),
    Piece::Value(0),
    Piece::Text("` calls, but it does not call `"),
    Piece::Value(0),
    Piece::Text("`. If the two do one job, call `"),
    Piece::Value(0),
    Piece::Text(
        "` and remove the copy. If they only use the same helpers, \
        ignore this row.",
    ),
]);

/// A template that takes (`path`, `target`, `line`, `listed`).
pub const CALLEE_OVERLAP_FOUND_EVIDENCE: Template<(Text, Text, Count, Text)> = Template::new(&[
    Piece::Text("`"),
    Piece::Value(1),
    Piece::Text("` at "),
    Piece::Value(0),
    Piece::Text(":"),
    Piece::Value(2),
    Piece::Text("; shared: "),
    Piece::Value(3),
]);

/// A template that takes (`path`, `reason`).
pub const CALLEE_OVERLAP_NEW_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: callee_overlap did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text after the change cannot be measured. "),
    Piece::Value(1),
]);

/// A template that takes (`path`, `reason`).
pub const CALLEE_OVERLAP_OLD_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: callee_overlap did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text before the change cannot be measured. "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const CALLEE_OVERLAP_READS: &str = "the calls of every function";

/// A template that takes (`path`, `detail`).
pub const CALLEE_OVERLAP_TOKENIZER_REJECTS: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": Python's tokenizer rejects this file ("),
    Piece::Value(1),
    Piece::Text(
        "). Functions are compared only in a file the tokenizer accepts. \
        Fix the line the error names, then run the check again.",
    ),
]);

/// A text that takes no values.
pub const CAPS_WALK_BYTES: &str = "note: max_walk_bytes counts only the files that a scan reads, \
        each with a suffix from the extensions list and at most \
        max_file_bytes bytes";

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

/// A template that takes (`count`, `total`, `modularity`, `resolution`).
pub const COMMUNITIES_HEADER: Template<(Count, Count, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "community", "communities"),
    Piece::Text(" over "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(" (modularity "),
    Piece::Value(2),
    Piece::Text(" at resolution "),
    Piece::Value(3),
    Piece::Text(")"),
]);

/// A text that takes no values.
pub const COMMUNITIES_NOT_CONVERGED: &str = "// note: community detection stopped at its pass \
        limit before the partition settled, so these communities are a \
        partial answer";

/// A text that takes no values.
pub const COMMUNITIES_NOTHING_PARSED: &str = "no communities: no file in this repository parsed";

/// A template that takes (`count`, `limit`).
pub const COMMUNITIES_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "community", "communities"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A text that takes no values.
pub const COMMUNITIES_WEAK: &str = "// note: weak partition; use these communities as a hint, not \
        a boundary";

/// A template that takes (`count`, `limit`).
pub const COMMUNITY_FILES_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(" in this community not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

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
        Piece::Text(" source "),
        Piece::Choice(4, "line", "lines"),
        Piece::Text(". This change takes the function over the threshold ("),
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
    Piece::Text(" source "),
    Piece::Choice(4, "line", "lines"),
    Piece::Text(". This function is already above the threshold ("),
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
    Piece::Text(" source "),
    Piece::Choice(2, "line", "lines"),
    Piece::Text(
        ". This function is new, or moved from another file, and it is \
        above the threshold (",
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
    Piece::Text(" "),
    Piece::Choice(
        1,
        "entry is not a repo-relative glob",
        "entries are not repo-relative globs",
    ),
    Piece::Text(" of at most "),
    Piece::Value(2),
    Piece::Text(" characters. "),
    Piece::Choice(1, "It is", "They are"),
    Piece::Text(" ignored."),
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
    Piece::Text(" "),
    Piece::Choice(2, "entry that names", "entries that name"),
    Piece::Text(" no gated check ("),
    Piece::Value(1),
    Piece::Text("). "),
    Piece::Choice(2, "It is", "They are"),
    Piece::Text(" ignored."),
]);

/// A template that takes (`config`, `count`, `chars`).
pub const CONFIG_STOPLIST_DROPPED: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("stoplist in "),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "entry is not a bare name", "entries are not bare names"),
    Piece::Text(" of at most "),
    Piece::Value(2),
    Piece::Text(" characters and "),
    Piece::Choice(1, "was", "were"),
    Piece::Text(" dropped"),
]);

/// A template that takes (`config`, `count`).
pub const CONFIG_UNKNOWN_KEYS_SUPPRESSED: Template<(Text, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" further unknown "),
    Piece::Choice(1, "key", "keys"),
    Piece::Text(" in "),
    Piece::Value(0),
    Piece::Text(": warnings suppressed"),
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

/// A text that takes no values.
pub const CYCLE_DODGE_COMPARED: &str = "no import";

/// A template that takes (`path`, `line`).
pub const CYCLE_DODGE_DOES_NOT_PARSE: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" does not parse: the first syntax error is at line "),
    Piece::Value(1),
    Piece::Text(
        ". Imports are compared only in a file that parses. Fix the \
        syntax error, then run the check again.",
    ),
]);

/// A template that takes (`module`, `function`, `chain`).
pub const CYCLE_DODGE_FOUND: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Text("The import of `"),
    Piece::Value(0),
    Piece::Text("` in `"),
    Piece::Value(1),
    Piece::Text(
        "` runs only when the function runs. At module level it closes \
        this import cycle: ",
    ),
    Piece::Value(2),
    Piece::Text(
        ". Break the cycle: move the shared code to a module that both \
        files import, or invert the dependency.",
    ),
]);

/// A template that takes (`path`, `target`, `hops`, `line`).
pub const CYCLE_DODGE_FOUND_EVIDENCE: Template<(Text, Text, Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(":"),
    Piece::Value(3),
    Piece::Text(" imports "),
    Piece::Value(1),
    Piece::Text("; "),
    Piece::Value(1),
    Piece::Text(" imports "),
    Piece::Value(0),
    Piece::Text(" back through "),
    Piece::Value(2),
]);

/// A template that takes (`path`, `reason`).
pub const CYCLE_DODGE_NEW_NOT_PARSED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: cycle_dodge did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text after the change does not parse. "),
    Piece::Value(1),
]);

/// A template that takes (`path`, `reason`).
pub const CYCLE_DODGE_OLD_NOT_PARSED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: cycle_dodge did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text before the change does not parse. "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const CYCLE_DODGE_READS: &str = "the imports of every file";

/// A template that takes `count`.
pub const CYCLES_HEADER: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" import "),
    Piece::Choice(0, "cycle", "cycles"),
]);

/// A template that takes (`count`, `limit`).
pub const CYCLES_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "cycle", "cycles"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(", shortest chain first)"),
]);

/// A template that takes `count`.
pub const CYCLES_UNRESOLVED: Template<Count> = Template::new(&[
    Piece::Text("// note: "),
    Piece::Value(0),
    Piece::Text(" import "),
    Piece::Choice(0, "statement", "statements"),
    Piece::Text(
        " of this repository's own modules did not resolve, so a cycle \
        through ",
    ),
    Piece::Choice(0, "it", "them"),
    Piece::Text(" is not listed"),
]);

/// A template that takes (`target`, `name`).
pub const DATA_KEY_NO_FAN_IN: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        " is a data key, and a data key has no fan-in: Grep finds the \
        spellings of ",
    ),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes (`count`, `limit`).
pub const DIAGRAM_EDGES_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("%% "),
    Piece::Value(0),
    Piece::Text(" lighter reference "),
    Piece::Choice(0, "edge", "edges"),
    Piece::Text(" not drawn (edge bound "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const DIAGRAM_ELIDED: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "module", "modules"),
]);

/// A text that takes no values.
pub const DIAGRAM_NOT_CONVERGED: &str = "the ranking that chose these modules did not converge, \
        so which ones the bound kept is a partial answer";

/// A template that takes (`count`, `total`).
pub const DIAGRAM_SUMMARY: Template<(Count, Count)> = Template::new(&[
    Piece::Text("diagram of "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "module", "modules"),
    Piece::Text("; "),
    Piece::Value(1),
    Piece::Text(" elided"),
]);

/// A template that takes (`target`, `count`, `total`).
pub const DIAGRAM_SUMMARY_FOCUS: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("diagram of "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "module", "modules"),
    Piece::Text(" around "),
    Piece::Value(0),
    Piece::Text("; "),
    Piece::Value(2),
    Piece::Text(" elided"),
]);

/// A template that takes (`target`, `count`, `total`).
pub const DIAGRAM_SUMMARY_FOCUS_GROUPED: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("diagram of "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "module", "modules"),
    Piece::Text(" around "),
    Piece::Value(0),
    Piece::Text(", grouped by community; "),
    Piece::Value(2),
    Piece::Text(" elided"),
]);

/// A template that takes (`count`, `total`).
pub const DIAGRAM_SUMMARY_GROUPED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("diagram of "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "module", "modules"),
    Piece::Text(", grouped by community; "),
    Piece::Value(1),
    Piece::Text(" elided"),
]);

/// A template that takes `count`.
pub const DIAGRAM_TITLES: Template<Count> = Template::new(&[
    Piece::Text("subgraph titles name whole communities, including the "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "module", "modules"),
    Piece::Text(" the rank bound left out of this diagram"),
]);

/// A template that takes (`shortened`, `total`, `budget`).
pub const EXPAND_BATCH_SHORTENED: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("// note: "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" bodies "),
    Piece::Choice(0, "was", "were"),
    Piece::Text(" shortened to fit the "),
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

/// A template that takes `limit`.
pub const EXPAND_OVER_LIMIT: Template<Count> = Template::new(&[
    Piece::Text("not expanded: the per-call limit is "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "symbol", "symbols"),
]);

/// A template that takes (`count`, `total`).
pub const EXPAND_UNRESOLVED_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("unresolved: "),
    Piece::Value(0),
    Piece::Text(" further "),
    Piece::Choice(0, "id is", "ids are"),
    Piece::Text(" not listed; "),
    Piece::Value(1),
    Piece::Text(" of the ids requested did not resolve"),
]);

/// A template that takes (`count`, `limit`).
pub const EXPLAIN_DEFINITIONS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "definition", "definitions"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const EXPLAIN_EDGES_UNLISTED: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "edge", "edges"),
    Piece::Text(" at this tier not listed"),
]);

/// A template that takes (`count`, `limit`).
pub const EXPLAIN_IMPORTERS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "importer", "importers"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `limit`).
pub const EXPLAIN_IMPORTS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more declared "),
    Piece::Choice(0, "import", "imports"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
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

/// A template that takes (`path`, `count`).
pub const FIND_DATA_FOLDED: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(" data keys match. Grep finds the spellings."),
]);

/// A template that takes (`path`, `count`).
pub const FIND_DATA_FOLDED_REST: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(" more data keys match. Grep finds the spellings."),
]);

/// A template that takes `count`.
pub const FIND_DATA_KIND_EXCLUDED: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" data "),
    Piece::Choice(0, "key matches", "keys match"),
    Piece::Text(
        ", and a kind filter leaves data keys out. Retry with no kind to \
        list ",
    ),
    Piece::Choice(0, "it", "them"),
    Piece::Text("."),
]);

/// A template that takes (`query`, `count`, `listed`).
pub const FIND_DATA_OUTSIDE: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Text("'"),
    Piece::Value(0),
    Piece::Text("' occurs in "),
    Piece::Value(1),
    Piece::Text(" data "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(" outside "),
    Piece::Choice(1, "its", "their"),
    Piece::Text(" outline: "),
    Piece::Value(2),
    Piece::Text(". Grep finds every spelling."),
]);

/// A template that takes (`count`, `budget`).
pub const FIND_DATA_OUTSIDE_UNREAD: Template<(Count, Count)> = Template::new(&[
    Piece::Text("note: the text search of data files stops at "),
    Piece::Value(1),
    Piece::Text(" bytes, so it did not read "),
    Piece::Value(0),
    Piece::Text(" data "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(". Grep reads every file."),
]);

/// A template that takes (`count`, `total`, `cap`, `listed`).
pub const FIND_DATA_UNLISTED: Template<(Count, Count, Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" data "),
    Piece::Choice(0, "key", "keys"),
    Piece::Text(" in "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(" "),
    Piece::Choice(0, "matches", "match"),
    Piece::Text(" and "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" not listed: "),
    Piece::Value(3),
    Piece::Text(". This answer has room for "),
    Piece::Value(2),
    Piece::Text(" data "),
    Piece::Choice(2, "entry", "entries"),
    Piece::Text(" after the code matches. Grep finds the spellings."),
]);

/// A template that takes `query`.
pub const FIND_NO_DECLARATION: Template<Text> = Template::new(&[
    Piece::Text("no declared symbol has '"),
    Piece::Value(0),
    Piece::Text("' in its name or qualified name, matched case-insensitively."),
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

/// A template that takes (`count`, `limit`).
pub const FIND_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "match", "matches"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`query`, `listed`).
pub const FIND_UNPARSED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("note: "),
    Piece::Value(0),
    Piece::Text(" is spelled where a file did not parse ("),
    Piece::Value(1),
    Piece::Text("), so a declaration there may be missing from this answer"),
]);

/// A template that takes (`name`, `count`).
pub const FIND_USAGE_HEAD: Template<(Text, Count)> = Template::new(&[
    Piece::Text("usage: "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file spells", "files spell"),
    Piece::Text(" '"),
    Piece::Value(0),
    Piece::Text("' and "),
    Piece::Choice(1, "does", "do"),
    Piece::Text(" not declare it"),
    Piece::Choice(1, ":", "."),
]);

/// A template that takes (`name`, `owner`, `count`).
pub const FIND_USAGE_HEAD_OWNER: Template<(Text, Text, Count)> = Template::new(&[
    Piece::Text("usage: "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "file spells", "files spell"),
    Piece::Text(" '"),
    Piece::Value(0),
    Piece::Text("' and '"),
    Piece::Value(1),
    Piece::Text("' and "),
    Piece::Choice(2, "does", "do"),
    Piece::Text(" not declare '"),
    Piece::Value(0),
    Piece::Text("'"),
    Piece::Choice(2, ":", "."),
]);

/// A template that takes `count`.
pub const FIND_USAGE_LINES: Template<Count> = Template::new(&[
    Piece::Text("The "),
    Piece::Value(0),
    Piece::Text(" with the most lines:"),
]);

/// A text that takes no values.
pub const FIND_USAGE_LINES_ALL: &str = "Most lines first:";

/// A template that takes `count`.
pub const FIND_USAGE_NEAREST: Template<Count> = Template::new(&[
    Piece::Text("The "),
    Piece::Value(0),
    Piece::Text(" nearest to a declaration:"),
]);

/// A text that takes no values.
pub const FIND_USAGE_NEAREST_ALL: &str = "Nearest to a declaration first:";

/// A template that takes (`name`, `count`).
pub const FIND_USAGE_REST: Template<(Text, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" more "),
    Piece::Choice(1, "file spells", "files spell"),
    Piece::Text(" '"),
    Piece::Value(0),
    Piece::Text("' and "),
    Piece::Choice(1, "is", "are"),
    Piece::Text(" not listed."),
]);

/// A template that takes (`name`, `owner`, `count`).
pub const FIND_USAGE_REST_OWNER: Template<(Text, Text, Count)> = Template::new(&[
    Piece::Value(2),
    Piece::Text(" more "),
    Piece::Choice(2, "file spells", "files spell"),
    Piece::Text(" '"),
    Piece::Value(0),
    Piece::Text("' and '"),
    Piece::Value(1),
    Piece::Text("' and "),
    Piece::Choice(2, "is", "are"),
    Piece::Text(" not listed."),
]);

/// A template that takes (`path`, `line`, `count`).
pub const FIND_USAGE_ROW: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("  "),
    Piece::Value(0),
    Piece::Text(" @"),
    Piece::Value(1),
    Piece::Text(" ("),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "line", "lines"),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const GROUPED_FILES: Template<Count> = Template::new(&[
    Piece::Text("("),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const GROUPED_IDS: Template<Count> = Template::new(&[
    Piece::Text("("),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "id", "ids"),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `total`, `threshold`, `over`, `erosion`, `share`).
pub const HEALTH_COMPLEXITY: Template<(Count, Count, Count, Count, Text, Text)> = Template::new(&[
    Piece::Text("complexity over "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "unit", "units"),
    Piece::Text(" in "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(" (threshold cc > "),
    Piece::Value(2),
    Piece::Text("): "),
    Piece::Value(3),
    Piece::Text(" over the threshold, erosion "),
    Piece::Value(4),
    Piece::Text(", clone share "),
    Piece::Value(5),
]);

/// A template that takes (`count`, `total`).
pub const HEALTH_COMPLEXITY_WALKED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("complexity: "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(" walked before the time budget"),
]);

/// A template that takes `count`.
pub const HEALTH_EXPORTS: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" unused "),
    Piece::Choice(0, "export", "exports"),
    Piece::Text(" (public function outside a function body, no counted edge in)"),
]);

/// A template that takes (`count`, `limit`).
pub const HEALTH_EXPORTS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more unused "),
    Piece::Choice(0, "export", "exports"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const HEALTH_HOTSPOTS: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "unit", "units"),
    Piece::Text(
        " ranked by mass (named functions and methods, mass = cc * \
        sqrt(sloc), highest first)",
    ),
]);

/// A template that takes (`count`, `limit`).
pub const HEALTH_HOTSPOTS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "hotspot", "hotspots"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const HEALTH_HUBS: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "symbol carries", "symbols carry"),
    Piece::Text(
        " a counted edge (function, method or class, highest counted \
        degree first)",
    ),
]);

/// A template that takes (`count`, `limit`).
pub const HEALTH_HUBS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "hub", "hubs"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const HEALTH_ORPHANS: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" orphan "),
    Piece::Choice(0, "candidate", "candidates"),
    Piece::Text(" (function, no counted edge in or out)"),
]);

/// A template that takes (`count`, `limit`).
pub const HEALTH_ORPHANS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more orphan "),
    Piece::Choice(0, "candidate", "candidates"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `count`.
pub const HEALTH_OVER: Template<Count> = Template::new(&[
    Piece::Text("health over "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "symbol", "symbols"),
]);

/// A template that takes (`count`, `total`).
pub const HEALTH_OVER_EXCLUDED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("health over "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "symbol", "symbols"),
    Piece::Text("; "),
    Piece::Value(1),
    Piece::Text(" excluded as test or fixture paths"),
]);

/// A template that takes `count`.
pub const HEALTH_UNMEASURED: Template<Count> = Template::new(&[
    Piece::Text("complexity: "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(" not measured"),
]);

/// A template that takes (`count`, `limit`).
pub const HEALTH_UNMEASURED_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more unmeasured "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

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

/// A template that takes (`path`, `target`, `start`, `end`, `count`).
pub const HISTORY_HEADER: Template<(Text, Text, Count, Count, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text("  "),
    Piece::Value(0),
    Piece::Text(":"),
    Piece::Value(2),
    Piece::Text("-"),
    Piece::Value(3),
    Piece::Text("  ("),
    Piece::Value(4),
    Piece::Text(" "),
    Piece::Choice(4, "commit", "commits"),
    Piece::Text(", newest first)"),
]);

/// A template that takes `shown`.
pub const HISTORY_MORE_COMMITS: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" newest "),
    Piece::Choice(0, "commit", "commits"),
    Piece::Text(" shown; older commits touch this span (raise limit for the rest)"),
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
    Piece::Text(" complete "),
    Piece::Choice(0, "commit", "commits"),
    Piece::Text(" above "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" shown"),
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

/// A template that takes (`path`, `command`, `error`).
pub const HOOK_INSTALL_BAD_JSON: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" is not valid JSON: "),
    Piece::Value(2),
    Piece::Text(". hook "),
    Piece::Value(1),
    Piece::Text(" wrote nothing."),
]);

/// A template that takes (`listed`, `added`, `replaced`, `removed`, `kept`).
pub const HOOK_INSTALL_COUNTS: Template<(Text, Count, Count, Count, Count)> = Template::new(&[
    Piece::Text("added "),
    Piece::Value(1),
    Piece::Text(", replaced "),
    Piece::Value(2),
    Piece::Text(", removed "),
    Piece::Value(3),
    Piece::Text(" retired"),
    Piece::Value(0),
    Piece::Text(", kept "),
    Piece::Value(4),
    Piece::Text(" other "),
    Piece::Choice(4, "handler", "handlers"),
]);

/// A template that takes (`path`, `summary`).
pub const HOOK_INSTALL_DRY_RUN: Template<(Text, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text("; --dry-run wrote nothing to "),
    Piece::Value(0),
]);

/// A text that takes no values.
pub const HOOK_INSTALL_HELP: &str = "usage: agentless-mcp hook install [--scope \
        user|project|local] [--repo PATH] [--server-name NAME] \
        [--dry-run]\n\nWrites the agentless hook entries of this binary \
        into one Claude Code settings file, and removes the retired \
        ones. Every other handler and key stays. --scope names the \
        file: user (the default), project or local. --repo names the \
        repository of a project or local file (the default is the \
        start-up directory). --server-name names the MCP server whose \
        trace calls ledger-mark follows (the default is agentless). \
        --dry-run prints the new file and writes nothing.";

/// A template that takes (`path`, `command`).
pub const HOOK_INSTALL_NOT_FINITE: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        " holds a number that JSON cannot write: NaN, Infinity, or a \
        number past the 64-bit float range, such as 1e400. hook ",
    ),
    Piece::Value(1),
    Piece::Text(" wrote nothing."),
]);

/// A template that takes (`path`, `command`).
pub const HOOK_INSTALL_RACED: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" changed twice while hook "),
    Piece::Value(1),
    Piece::Text(" wrote it. hook "),
    Piece::Value(1),
    Piece::Text(" wrote nothing. Run it again."),
]);

/// A template that takes `path`.
pub const HOOK_INSTALL_UNCHANGED: Template<Text> = Template::new(&[
    Piece::Text("no change: "),
    Piece::Value(0),
    Piece::Text(" holds the current entries"),
]);

/// A template that takes (`path`, `summary`).
pub const HOOK_INSTALL_WROTE: Template<(Text, Text)> =
    Template::new(&[Piece::Value(1), Piece::Text("; wrote "), Piece::Value(0)]);

/// A template that takes `name`.
pub const HOOK_STATUS_CURRENT: Template<Text> =
    Template::new(&[Piece::Value(0), Piece::Text(": current")]);

/// A text that takes no values.
pub const HOOK_STATUS_HELP: &str = "usage: agentless-mcp hook status [--repo PATH] [--server-name \
        NAME]\n\nCompares the agentless hook entries of the user, \
        project and local settings files with the entry table of this \
        binary, one line per entry. The exit code is 0 when every entry \
        is current, else 1.";

/// A template that takes `name`.
pub const HOOK_STATUS_MISSING: Template<Text> =
    Template::new(&[Piece::Value(0), Piece::Text(": missing")]);

/// A template that takes (`path`, `name`).
pub const HOOK_STATUS_OTHER_PATH: Template<(Text, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(": runs "),
    Piece::Value(0),
    Piece::Text(", not this binary"),
]);

/// A template that takes `name`.
pub const HOOK_STATUS_RETIRED: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": retired: hook install removes it"),
]);

/// A template that takes `name`.
pub const HOOK_STATUS_STALE: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": stale: the entry differs from the table of this binary"),
]);

/// A text that takes no values.
pub const HOOK_UNINSTALL_HELP: &str = "usage: agentless-mcp hook uninstall [--scope \
        user|project|local] [--repo PATH]\n\nRemoves every agentless \
        hook entry from one Claude Code settings file. Every other \
        handler and key stays.";

/// A template that takes `path`.
pub const HOOK_UNINSTALL_NONE: Template<Text> = Template::new(&[
    Piece::Text("nothing to remove: "),
    Piece::Value(0),
    Piece::Text(" holds no agentless entry"),
]);

/// A template that takes (`path`, `removed`, `kept`).
pub const HOOK_UNINSTALL_REPORT: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("removed "),
    Piece::Value(1),
    Piece::Text(" agentless "),
    Piece::Choice(1, "handler", "handlers"),
    Piece::Text(", kept "),
    Piece::Value(2),
    Piece::Text(" other "),
    Piece::Choice(2, "handler", "handlers"),
    Piece::Text("; wrote "),
    Piece::Value(0),
]);

/// A text that takes no values.
pub const HOOK_USAGE: &str = "hook takes one hook name: read-card, ledger-mark, brief-record, \
        subagent-map, prompt-map, erosion or ledger-reset. hook \
        install, hook status and hook uninstall manage the hook entries \
        of a settings file. gate-check and gate-mark are retired: each \
        still exits 0 with no output.";

/// A template that takes (`count`, `total`, `listed`).
pub const HOOKS_MISSING: Template<(Count, Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" agentless hook "),
    Piece::Choice(1, "entry", "entries"),
    Piece::Text(" "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" missing ("),
    Piece::Value(2),
    Piece::Text(")"),
]);

/// A template that takes `summary`.
pub const HOOKS_NOTE: Template<Text> = Template::new(&[
    Piece::Text("hooks: "),
    Piece::Value(0),
    Piece::Text(". Run: agentless-mcp hook install"),
]);

/// A template that takes (`file`, `path`).
pub const HOOKS_OTHER_PATH: Template<(Text, Text)> = Template::new(&[
    Piece::Text("hooks run "),
    Piece::Value(1),
    Piece::Text("; this server runs "),
    Piece::Value(0),
]);

/// A template that takes (`count`, `listed`).
pub const HOOKS_RETIRED: Template<(Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" agentless hook "),
    Piece::Choice(0, "entry is", "entries are"),
    Piece::Text(" retired ("),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `listed`).
pub const HOOKS_RETIRED_MORE: Template<(Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" retired ("),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`accepted`, `path`).
pub const HOOKS_SHAPE: Template<(Text, Text)> =
    Template::new(&[Piece::Value(1), Piece::Text(" is not "), Piece::Value(0)]);

/// A template that takes (`count`, `total`, `listed`).
pub const HOOKS_STALE: Template<(Count, Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" agentless hook "),
    Piece::Choice(1, "entry", "entries"),
    Piece::Text(" "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" stale ("),
    Piece::Value(2),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `listed`).
pub const HOOKS_STALE_MORE: Template<(Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" stale ("),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`path`, `error`).
pub const HOOKS_UNREADABLE: Template<(Text, Text)> = Template::new(&[
    Piece::Text("hooks: "),
    Piece::Value(0),
    Piece::Text(" is not readable: "),
    Piece::Value(1),
    Piece::Text("; hook state unknown"),
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

/// A template that takes (`check`, `reads`).
pub const LINT_TREE_CHECK_SINCE_ONLY: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" runs only with --since REV. It reads "),
    Piece::Value(1),
    Piece::Text(
        " in the working tree after the change, and only --since \
        compares a change that the working tree holds. Remove ",
    ),
    Piece::Value(0),
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

/// A template that takes (`name`, `listed`).
pub const LISTED_AND: Template<(Text, Text)> =
    Template::new(&[Piece::Value(1), Piece::Text(" and "), Piece::Value(0)]);

/// A template that takes (`name`, `listed`).
pub const LISTED_OR: Template<(Text, Text)> =
    Template::new(&[Piece::Value(1), Piece::Text(" or "), Piece::Value(0)]);

/// A template that takes (`path`, `count`).
pub const LISTING_ROW: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "line", "lines"),
]);

/// A template that takes (`path`, `count`, `total`).
pub const LISTING_ROW_SYMBOLS: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "line", "lines"),
    Piece::Text("  "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "symbol", "symbols"),
]);

/// A template that takes (`path`, `reason`).
pub const LISTING_ROW_UNREAD: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  not read: "),
    Piece::Value(1),
]);

/// A template that takes (`count`, `budget`).
pub const MAP_BUDGET_NONE: Template<(Count, Count)> = Template::new(&[
    Piece::Text("// note: the "),
    Piece::Value(1),
    Piece::Text("-token budget left room for none of the "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "symbol", "symbols"),
    Piece::Text("; raise --budget"),
]);

/// A template that takes `path`.
pub const MAP_COMPANION_UNLISTED: Template<Text> =
    Template::new(&[Piece::Value(0), Piece::Text(" (not listed)")]);

/// A template that takes `count`.
pub const MAP_FILE_UNREACHED: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "symbol", "symbols"),
    Piece::Text(
        " in this file; the focus has no reference path to it, so no \
        budget shows ",
    ),
    Piece::Choice(0, "it", "them"),
]);

/// A template that takes `count`.
pub const MAP_MAX_FILES_NONE: Template<Count> = Template::new(&[
    Piece::Text("// note: --max-files kept none of the "),
    Piece::Value(0),
    Piece::Text(" ranked "),
    Piece::Choice(0, "file", "files"),
]);

/// A text that takes no values.
pub const MAP_NOT_CONVERGED: &str = "// note: the ranking behind this map did not converge, so \
        the order of the files below is a partial answer";

/// A text that takes no values.
pub const MAP_NOTHING_PARSED: &str = "// note: no file in this repository parsed";

/// A template that takes (`count`, `total`, `budget`).
pub const MAP_OVER_BUDGET: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("// note: this map renders to "),
    Piece::Value(1),
    Piece::Text(" tokens against a "),
    Piece::Value(2),
    Piece::Text("-token budget: the "),
    Piece::Value(0),
    Piece::Text(" ranked "),
    Piece::Choice(0, "file is", "files are"),
    Piece::Text(
        " listed whatever the budget allows, and the budget bounds only \
        the symbols shown under ",
    ),
    Piece::Choice(0, "it", "them"),
]);

/// A template that takes `count`.
pub const MAP_SYMBOLS_UNLISTED: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "symbol", "symbols"),
    Piece::Text(" in this file not listed"),
]);

/// A template that takes (`count`, `limit`).
pub const MAP_TESTS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more test "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
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

/// A text that takes no values.
pub const MAP_WALK_CAPPED: &str = "// note: the walk out to these tests hit its node bound before \
        it finished, so this section is a floor rather than every test";

/// A template that takes (`count`, `cap`).
pub const OVERVIEW_DATA_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "key is", "keys are"),
    Piece::Text(" not listed: the outline stops at depth 2, at list items and at "),
    Piece::Value(1),
    Piece::Text(" keys. Grep finds "),
    Piece::Choice(0, "it", "them"),
    Piece::Text("."),
]);

/// A template that takes `pattern`.
pub const OVERVIEW_STABLE_IDS: Template<Text> = Template::new(&[
    Piece::Text("stable ids: "),
    Piece::Value(0),
    Piece::Text(" -- nested symbols qualify as Class.method"),
]);

/// A template that takes `pattern`.
pub const OVERVIEW_STABLE_IDS_DATA: Template<Text> = Template::new(&[
    Piece::Text("stable ids: "),
    Piece::Value(0),
    Piece::Text(" -- nested keys qualify as parent.child"),
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

/// A template that takes (`target`, `name`).
pub const PATH_DATA_KEY_NO_EDGES: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        " is a data key, and a data key has no edges: Grep finds the \
        spellings of ",
    ),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes `target`.
pub const PATH_ENDPOINT_NO_MATCH: Template<Text> =
    Template::new(&[Piece::Text("no symbol or file matches "), Piece::Value(0)]);

/// A template that takes `tool`.
pub const PATH_NEEDS_ENDPOINTS: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        " operation 'path' needs both source and target: each is a \
        stable id, a qualified name, or a repository-relative file \
        path. Retry naming both ends.",
    ),
]);

/// A template that takes (`name`, `count`, `listed`).
pub const PATH_SOURCE_CANDIDATES: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "definition", "definitions"),
    Piece::Text(" of "),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(2),
    Piece::Text(". Pass "),
    Piece::Choice(1, "it", "one"),
    Piece::Text(" as the source."),
]);

/// A template that takes (`name`, `shown`, `count`, `listed`).
pub const PATH_SOURCE_CANDIDATES_CUT: Template<(Text, Count, Count, Text)> = Template::new(&[
    Piece::Value(2),
    Piece::Text(" definitions of "),
    Piece::Value(0),
    Piece::Text(", and "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "is", "are"),
    Piece::Text(" listed: "),
    Piece::Value(3),
    Piece::Text(". Pass one as the source."),
]);

/// A template that takes (`paths`, `seeds`).
pub const PROMPT_MAP_CONTEXT: Template<(Text, Text)> = Template::new(&[
    Piece::Text("Repository map (agentless-mcp): for "),
    Piece::Value(1),
    Piece::Text(" in this task, the files that rank first are "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`path`, `card`).
pub const READ_CARD_CONTEXT: Template<(Text, Text)> = Template::new(&[
    Piece::Text("File card (agentless-mcp), first read of "),
    Piece::Value(0),
    Piece::Text(" in this context:\n"),
    Piece::Value(1),
]);

/// A template that takes (`path`, `cap`).
pub const READ_CARD_CUT: Template<(Text, Count)> = Template::new(&[
    Piece::Text("(card cut at "),
    Piece::Value(1),
    Piece::Text(" characters; trace "),
    Piece::Value(0),
    Piece::Text(" for the rest)"),
]);

/// A template that takes `target`.
pub const REFS_ABSENT: Template<Text> = Template::new(&[
    Piece::Text("no references to "),
    Piece::Value(0),
    Piece::Text(" outside its own definition"),
]);

/// A template that takes (`target`, `count`).
pub const REFS_ABSENT_COMPLETE: Template<(Text, Count)> = Template::new(&[
    Piece::Text("no references to "),
    Piece::Value(0),
    Piece::Text(" outside its own definition (complete scan: "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(", 0 skipped)"),
]);

/// A template that takes (`target`, `count`, `total`).
pub const REFS_ABSENT_PARTIAL: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("no references to "),
    Piece::Value(0),
    Piece::Text(" outside its own definition in the "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(" read (partial scan: "),
    Piece::Value(2),
    Piece::Text(" code "),
    Piece::Choice(2, "file", "files"),
    Piece::Text(" skipped)"),
]);

/// A template that takes (`count`, `limit`).
pub const REFS_CALLERS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "caller", "callers"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `limit`).
pub const REFS_CANDIDATES_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "candidate", "candidates"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`path`, `count`, `note`).
pub const REFS_GROUP_HEADER: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  ("),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "reference", "references"),
    Piece::Value(2),
    Piece::Text(")"),
]);

/// A template that takes (`target`, `count`).
pub const REFS_HEADER: Template<(Text, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "reference", "references"),
    Piece::Text(" to "),
    Piece::Value(0),
]);

/// A template that takes (`name`, `count`, `total`).
pub const REFS_HEADER_SHARED: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "reference", "references"),
    Piece::Text(" to "),
    Piece::Value(0),
    Piece::Text(", a name that "),
    Piece::Value(2),
    Piece::Text(" definitions share"),
]);

/// A template that takes (`target`, `count`, `total`, `score`).
pub const REFS_SHARED_CANDIDATE: Template<(Text, Count, Count, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  ("),
    Piece::Value(1),
    Piece::Text(" shared "),
    Piece::Choice(1, "caller", "callers"),
    Piece::Text(" in "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "file", "files"),
    Piece::Text(", score "),
    Piece::Value(3),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `total`).
pub const REFS_SPLIT: Template<(Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "row has", "rows have"),
    Piece::Text(" binding evidence (same-file or resolved-via-import). "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "is a candidate", "are candidates"),
    Piece::Text(" (unique or name-only-ambiguous)."),
]);

/// A template that takes `count`.
pub const REFS_SPLIT_NONE: Template<Count> = Template::new(&[
    Piece::Text("no row has binding evidence: "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "is a candidate", "are candidates"),
    Piece::Text(" (unique or name-only-ambiguous)."),
]);

/// A template that takes (`count`, `limit`).
pub const REFS_UNLISTED: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "reference", "references"),
    Piece::Text(" not listed (limit "),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes (`count`, `total`, `limit`).
pub const REFS_UNLISTED_FILES: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "reference", "references"),
    Piece::Text(" not listed (limit "),
    Piece::Value(2),
    Piece::Text("), including every reference in "),
    Piece::Value(1),
    Piece::Text(" more "),
    Piece::Choice(1, "file", "files"),
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

/// A text that takes no values.
pub const REUSE_CANDIDATES_COMPARED: &str = "no function";

/// A template that takes (`path`, `target`, `name`, `line`, `count`, `listed`).
pub const REUSE_CANDIDATES_FOUND: Template<(Text, Text, Text, Count, Count, Text)> =
    Template::new(&[
        Piece::Text("`"),
        Piece::Value(1),
        Piece::Text("` at "),
        Piece::Value(0),
        Piece::Text(":"),
        Piece::Value(3),
        Piece::Text(" calls and reads "),
        Piece::Value(4),
        Piece::Text(" of the same "),
        Piece::Choice(4, "name", "names"),
        Piece::Text(" as `"),
        Piece::Value(2),
        Piece::Text("`: "),
        Piece::Value(5),
        Piece::Text(". Read `"),
        Piece::Value(1),
        Piece::Text("` before you keep a second copy."),
    ]);

/// A template that takes `score`.
pub const REUSE_CANDIDATES_FOUND_EVIDENCE: Template<Text> =
    Template::new(&[Piece::Text("score "), Piece::Value(0)]);

/// A template that takes (`paths`, `count`, `total`).
pub const REUSE_CANDIDATES_GAPS: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("not checked: reuse_candidates did not read "),
    Piece::Value(1),
    Piece::Text(" of "),
    Piece::Value(2),
    Piece::Text(
        " repository files, so it does not compare a function with the \
        functions in them: ",
    ),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`path`, `reason`).
pub const REUSE_CANDIDATES_GAPS_EVIDENCE: Template<(Text, Text)> = Template::new(&[
    Piece::Text("first: "),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
]);

/// A template that takes (`path`, `reason`).
pub const REUSE_CANDIDATES_NEW_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: reuse_candidates did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text after the change cannot be measured. "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const REUSE_CANDIDATES_NO_TAG_CACHE: &str = "not checked: in the hook form, reuse_candidates \
        reads the functions of the repository from the tag cache only, \
        and this run has no tag cache. Run agentless-mcp index on the \
        repository to turn the check on.";

/// A text that takes no values.
pub const REUSE_CANDIDATES_NO_TAG_CACHE_EVIDENCE: &str = "no tag cache";

/// A template that takes (`path`, `reason`).
pub const REUSE_CANDIDATES_OLD_NOT_MEASURED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("not checked: reuse_candidates did not compare "),
    Piece::Value(0),
    Piece::Text(", because its text before the change cannot be measured. "),
    Piece::Value(1),
]);

/// A text that takes no values.
pub const REUSE_CANDIDATES_READS: &str = "the action names of every function";

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
    Piece::Text(" "),
    Piece::Choice(0, "file was", "files were"),
    Piece::Text(" skipped and "),
    Piece::Choice(0, "is", "are"),
    Piece::Text(" absent from this answer: "),
    Piece::Value(1),
    Piece::Text(
        ". A symbol in a skipped file reads as missing here even though \
        it may exist in the repository.",
    ),
]);

/// A template that takes (`pattern`, `count`, `reason`).
pub const SEARCH_BAD_PATTERN: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Text("pattern is not a valid regular expression: "),
    Piece::Value(2),
    Piece::Text(" at column "),
    Piece::Value(1),
    Piece::Text(". To match the text, pass pattern=\""),
    Piece::Value(0),
    Piece::Text("\""),
]);

/// A template that takes (`pattern`, `reason`).
pub const SEARCH_BAD_PATTERN_NO_COLUMN: Template<(Text, Text)> = Template::new(&[
    Piece::Text("pattern is not a valid regular expression: "),
    Piece::Value(1),
    Piece::Text(". To match the text, pass pattern=\""),
    Piece::Value(0),
    Piece::Text("\""),
]);

/// A template that takes (`chars`, `text`).
pub const SEARCH_CUT: Template<(Count, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text("...[+"),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "char", "chars"),
    Piece::Text("]"),
]);

/// A template that takes (`query`, `count`, `glob`).
pub const SEARCH_LISTING: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Text("search files="),
    Piece::Value(2),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
]);

/// A template that takes (`shown`, `total`).
pub const SEARCH_LISTING_REST: Template<(Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" files shown: raise limit for more."),
]);

/// A template that takes `count`.
pub const SEARCH_MORE_FILES: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" more "),
    Piece::Choice(0, "file", "files"),
]);

/// A template that takes (`kind`, `count`, `total`, `listed`).
pub const SEARCH_NOISE: Template<(Text, Count, Count, Text)> = Template::new(&[
    Piece::Text("+ "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "match", "matches"),
    Piece::Text(" in "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(2, "file", "files"),
    Piece::Text(": "),
    Piece::Value(3),
    Piece::Text(". path=<file> shows them."),
]);

/// A text that takes no values.
pub const SEARCH_NOTHING_ASKED: &str = "search needs pattern, files or both.";

/// A template that takes (`pattern`, `limit`).
pub const SEARCH_PATTERN_TOO_BIG: Template<(Text, Count)> = Template::new(&[
    Piece::Text(
        "pattern is not a valid regular expression: it compiles to more \
        than ",
    ),
    Piece::Value(1),
    Piece::Text(" bytes. To match the text, pass pattern=\""),
    Piece::Value(0),
    Piece::Text("\""),
]);

/// A template that takes (`count`, `listed`).
pub const SEARCH_REPEAT: Template<(Count, Text)> = Template::new(&[
    Piece::Text("same line in "),
    Piece::Value(0),
    Piece::Text(" places: "),
    Piece::Value(1),
]);

/// A template that takes (`shown`, `count`, `total`, `listed`).
pub const SEARCH_REST: Template<(Count, Count, Count, Text)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(2),
    Piece::Text(" match lines shown; the rest are in "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(": "),
    Piece::Value(3),
]);

/// A template that takes `glob`.
pub const SEARCH_SCOPE_FILES: Template<Text> =
    Template::new(&[Piece::Text(" files="), Piece::Value(0)]);

/// A template that takes `path`.
pub const SEARCH_SCOPE_PATH: Template<Text> =
    Template::new(&[Piece::Text(" in "), Piece::Value(0)]);

/// A template that takes (`count`, `listed`).
pub const SEARCH_SKIPPED: Template<(Count, Text)> = Template::new(&[
    Piece::Text("// warning: "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "file was", "files were"),
    Piece::Text(" not searched: "),
    Piece::Value(1),
]);

/// A template that takes (`query`, `pattern`, `count`, `total`).
pub const SEARCH_SUMMARY: Template<(Text, Text, Count, Count)> = Template::new(&[
    Piece::Text("search /"),
    Piece::Value(1),
    Piece::Text("/"),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "match", "matches"),
    Piece::Text(" in "),
    Piece::Value(3),
    Piece::Text(" "),
    Piece::Choice(3, "file", "files"),
]);

/// A template that takes (`query`, `pattern`, `total`).
pub const SEARCH_SUMMARY_NONE: Template<(Text, Text, Count)> = Template::new(&[
    Piece::Text("search /"),
    Piece::Value(1),
    Piece::Text("/"),
    Piece::Value(0),
    Piece::Text(": 0 matches in "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "file", "files"),
    Piece::Text(" searched."),
]);

/// A template that takes (`limit`, `glob`).
pub const SEARCH_WIDE_GLOB: Template<(Count, Text)> = Template::new(&[
    Piece::Text("files expands to more than "),
    Piece::Value(0),
    Piece::Text(" globs: "),
    Piece::Value(1),
    Piece::Text(". Use fewer brace groups."),
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

/// A text that takes no values.
pub const SLICE_LINES_AND_WHOLE_FILE: &str = "Operation slice takes lines or whole_file=true, but \
        not both.";

/// A template that takes `accepted`.
pub const SLICE_NEEDS_LINES: Template<Text> = Template::new(&[
    Piece::Text(
        "Operation slice requires lines or whole_file=true, and lines \
        must be ",
    ),
    Piece::Value(0),
    Piece::Text("."),
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
    Piece::Text(" "),
    Piece::Choice(3, "line", "lines"),
    Piece::Text(")"),
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

/// A template that takes `found`.
pub const SLICE_RANGE_REVERSED: Template<Text> = Template::new(&[
    Piece::Text("lines holds "),
    Piece::Value(0),
    Piece::Text(", but each pair must have its end at or after its start."),
]);

/// A template that takes (`path`, `qualname`, `cap`).
pub const SPAN_DATA_KEY_UNLISTED: Template<(Text, Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" keeps an outline of its keys, and "),
    Piece::Value(1),
    Piece::Text(
        " is not in it: the outline stops at depth 2, at list items and \
        at ",
    ),
    Piece::Value(2),
    Piece::Text(" keys. Read the lines with read slice, or Grep for the key."),
]);

/// A template that takes (`path`, `qualname`).
pub const SPAN_SYMBOL_UNDEFINED: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" does not define "),
    Piece::Value(1),
]);

/// A template that takes `pattern`.
pub const STABLE_IDS_PATTERN: Template<Text> =
    Template::new(&[Piece::Text("stable ids: "), Piece::Value(0)]);

/// A template that takes (`paths`, `seeds`).
pub const SUBAGENT_MAP_CONTEXT: Template<(Text, Text)> = Template::new(&[
    Piece::Text("Repository map (agentless-mcp): for "),
    Piece::Value(1),
    Piece::Text(
        " in this brief, the files that rank first and that the brief \
        does not name are ",
    ),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A text that takes no values.
pub const SURFACE_REFUSED: &str = "--surface is no longer served: this release publishes trace \
        and search only. Drop --surface.";

/// A template that takes (`name`, `count`, `listed`).
pub const TARGET_CANDIDATES: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "definition", "definitions"),
    Piece::Text(" of "),
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(2),
    Piece::Text(". Pass "),
    Piece::Choice(1, "it", "one"),
    Piece::Text(" as the target."),
]);

/// A template that takes (`name`, `shown`, `count`, `listed`).
pub const TARGET_CANDIDATES_CUT: Template<(Text, Count, Count, Text)> = Template::new(&[
    Piece::Value(2),
    Piece::Text(" definitions of "),
    Piece::Value(0),
    Piece::Text(", and "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "is", "are"),
    Piece::Text(" listed: "),
    Piece::Value(3),
    Piece::Text(". Pass one as the target."),
]);

/// A template that takes `path`.
pub const TARGET_DATA_FILE: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        " is a data file, and its keys are not symbols that refs, \
        explain or path bind",
    ),
]);

/// A template that takes (`target`, `line`).
pub const TARGET_DATA_KEY: Template<(Text, Count)> = Template::new(&[
    Piece::Text("The key "),
    Piece::Value(0),
    Piece::Text(" spans line "),
    Piece::Value(1),
    Piece::Text(". Expand that id to read the key."),
]);

/// A template that takes (`path`, `reason`).
pub const TARGET_FILE_SKIPPED: Template<(Text, Text)> = Template::new(&[
    Piece::Text("the scan did not read "),
    Piece::Value(0),
    Piece::Text(" ("),
    Piece::Value(1),
    Piece::Text(")"),
]);

/// A template that takes `target`.
pub const TARGET_IS_FILE: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" is a file. Pass the stable id of one of its symbols"),
]);

/// A template that takes `path`.
pub const TARGET_NO_FILE: Template<Text> =
    Template::new(&[Piece::Text("the scan read no file "), Piece::Value(0)]);

/// A template that takes (`path`, `line`).
pub const TARGET_NO_LINE: Template<(Text, Count)> = Template::new(&[
    Piece::Text("no symbol in "),
    Piece::Value(0),
    Piece::Text(" spans line "),
    Piece::Value(1),
]);

/// A template that takes `target`.
pub const TARGET_NO_MATCH: Template<Text> =
    Template::new(&[Piece::Text("no symbol matches "), Piece::Value(0)]);

/// A template that takes (`name`, `count`).
pub const TARGET_SPELLED: Template<(Text, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file spells", "files spell"),
    Piece::Text(" "),
    Piece::Value(0),
    Piece::Text(" in code, and no file declares it. Grep finds every spelling."),
]);

/// A template that takes (`tool`, `accepted`).
pub const TOOL_TAKES: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" takes "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes `count`.
pub const TRACE_AT_LEAST: Template<Count> =
    Template::new(&[Piece::Text("at least "), Piece::Value(0)]);

/// A template that takes `count`.
pub const TRACE_BOUND: Template<Count> = Template::new(&[Piece::Value(0), Piece::Text(" bound")]);

/// A template that takes (`target`, `count`).
pub const TRACE_CANDIDATES: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": "),
    Piece::Value(1),
    Piece::Text(" definitions. Trace one by its id."),
]);

/// A template that takes `listed`.
pub const TRACE_CYCLE_MEMBERS: Template<Text> =
    Template::new(&[Piece::Text("also in this cycle: "), Piece::Value(0)]);

/// A template that takes (`count`, `total`).
pub const TRACE_CYCLES: Template<(Count, Count)> = Template::new(&[
    Piece::Text("in "),
    Piece::Value(0),
    Piece::Text(" import "),
    Piece::Choice(0, "cycle", "cycles"),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
]);

/// A template that takes (`kinds`, `count`).
pub const TRACE_DECLARES: Template<(Text, Count)> = Template::new(&[
    Piece::Value(1),
    Piece::Text(" ("),
    Piece::Value(0),
    Piece::Text(")"),
]);

/// A template that takes (`path`, `language`, `pattern`, `count`).
pub const TRACE_FILE_HEADER: Template<(Text, Text, Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  "),
    Piece::Value(1),
    Piece::Text("  "),
    Piece::Value(3),
    Piece::Text(" "),
    Piece::Choice(3, "line", "lines"),
    Piece::Text("  ids: "),
    Piece::Value(2),
]);

/// A template that takes (`path`, `count`, `total`).
pub const TRACE_FOLDER_HEADER: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("/  "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(", "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Choice(2, "line", "lines"),
]);

/// A template that takes (`path`, `count`).
pub const TRACE_FOLDER_ROW: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("/  "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
]);

/// A template that takes `count`.
pub const TRACE_HIDDEN: Template<Count> = Template::new(&[Piece::Value(0), Piece::Text(" hidden")]);

/// A template that takes (`sha`, `date`, `subject`).
pub const TRACE_LAST_CHANGE: Template<(Text, Text, Text)> = Template::new(&[
    Piece::Text("last change: "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Value(2),
]);

/// A template that takes `count`.
pub const TRACE_LAST_CHANGE_NOT_READ: Template<Count> = Template::new(&[
    Piece::Text("last change: not read in "),
    Piece::Value(0),
    Piece::Text(" s"),
]);

/// A template that takes `reason`.
pub const TRACE_LAST_CHANGE_UNAVAILABLE: Template<Text> =
    Template::new(&[Piece::Text("last change: not read: "), Piece::Value(0)]);

/// A template that takes `count`.
pub const TRACE_LINE_PAST_END: Template<Count> = Template::new(&[
    Piece::Text(": the file has "),
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "line", "lines"),
    Piece::Text("."),
]);

/// A template that takes (`pattern`, `count`, `listed`).
pub const TRACE_LITERAL: Template<(Text, Count, Text)> = Template::new(&[
    Piece::Text("It occurs as text in "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
    Piece::Text(": "),
    Piece::Value(2),
    Piece::Text(". search(pattern=\""),
    Piece::Value(0),
    Piece::Text("\") lists every match."),
]);

/// A text that takes no values.
pub const TRACE_LITERAL_NONE: &str = "No file holds it as text.";

/// A template that takes `name`.
pub const TRACE_MORE_HINT: Template<Text> =
    Template::new(&[Piece::Text("(more="), Piece::Value(0), Piece::Text(")")]);

/// A template that takes `count`.
pub const TRACE_NAME_ONLY: Template<Count> =
    Template::new(&[Piece::Value(0), Piece::Text(" name-only")]);

/// A template that takes (`count`, `total`).
pub const TRACE_NAME_ONLY_FILES: Template<(Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" name-only in "),
    Piece::Value(1),
    Piece::Text(" "),
    Piece::Choice(1, "file", "files"),
]);

/// A template that takes (`count`, `total`).
pub const TRACE_NAME_ONLY_FILES_FLOOR: Template<(Count, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" name-only in at least "),
    Piece::Value(1),
    Piece::Text(" files"),
]);

/// A text that takes no values.
pub const TRACE_NO_ANCHOR: &str = "no symbol of this file joins the name graph";

/// A template that takes (`path`, `kind`).
pub const TRACE_NO_GRAMMAR: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": no grammar reads "),
    Piece::Value(1),
    Piece::Text(" files. Read it or search it."),
]);

/// A template that takes `path`.
pub const TRACE_NO_GRAMMAR_NAME: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": no grammar reads this file. Read it or search it."),
]);

/// A template that takes `target`.
pub const TRACE_NO_MATCH: Template<Text> = Template::new(&[
    Piece::Text("no symbol, file or folder matches "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`name`, `listed`).
pub const TRACE_NO_SECTION_FILE: Template<(Text, Text)> = Template::new(&[
    Piece::Text("more="),
    Piece::Value(0),
    Piece::Text(" does not apply to a file card. File sections: "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes (`name`, `listed`).
pub const TRACE_NO_SECTION_FOLDER: Template<(Text, Text)> = Template::new(&[
    Piece::Text("more="),
    Piece::Value(0),
    Piece::Text(" does not apply to a folder card. Folder sections: "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes (`name`, `listed`).
pub const TRACE_NO_SECTION_SYMBOL: Template<(Text, Text)> = Template::new(&[
    Piece::Text("more="),
    Piece::Value(0),
    Piece::Text(" does not apply to a symbol card. Symbol sections: "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A text that takes no values.
pub const TRACE_NO_SYMBOL_AT: &str = "; this is the file card.";

/// A template that takes (`path`, `reason`).
pub const TRACE_NOT_PARSED: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" is not parsed: "),
    Piece::Value(1),
    Piece::Text(". Read it or search it."),
]);

/// A template that takes `count`.
pub const TRACE_NOT_RESOLVED: Template<Count> = Template::new(&[
    Piece::Text("not resolved: the scan skipped "),
    Piece::Value(0),
    Piece::Text(" code "),
    Piece::Choice(0, "file", "files"),
    Piece::Text(", so a caller can be missing"),
]);

/// A template that takes `path`.
pub const TRACE_NOT_SCANNED: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": the scan does not read this file. Read it or search it."),
]);

/// A template that takes `text`.
pub const TRACE_PART_HIDDEN: Template<Text> =
    Template::new(&[Piece::Value(0), Piece::Text(" hidden")]);

/// A template that takes `count`.
pub const TRACE_ROWS_UNLISTED: Template<Count> =
    Template::new(&[Piece::Text("... "), Piece::Value(0), Piece::Text(" more")]);

/// A template that takes `count`.
pub const TRACE_ROWS_UNLISTED_FLOOR: Template<Count> = Template::new(&[
    Piece::Text("... at least "),
    Piece::Value(0),
    Piece::Text(" more"),
]);

/// A template that takes `count`.
pub const TRACE_SAME_CARD: Template<Count> =
    Template::new(&[Piece::Text("same card as target "), Piece::Value(0)]);

/// A template that takes `name`.
pub const TRACE_SECTION_NONE: Template<Text> =
    Template::new(&[Piece::Value(0), Piece::Text(": none")]);

/// A template that takes (`name`, `reason`).
pub const TRACE_SECTION_UNREAD: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(": not read: "),
    Piece::Value(1),
]);

/// A template that takes `count`.
pub const TRACE_SIMILAR: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "candidate", "candidates"),
]);

/// A template that takes `count`.
pub const TRACE_SIMILAR_NAME_ONLY: Template<Count> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" "),
    Piece::Choice(0, "candidate", "candidates"),
    Piece::Text(", ranked by name-only callers; hidden"),
]);

/// A template that takes (`target`, `count`).
pub const TRACE_SIMILAR_ROW: Template<(Text, Count)> = Template::new(&[
    Piece::Value(0),
    Piece::Text("  "),
    Piece::Value(1),
    Piece::Text(" shared "),
    Piece::Choice(1, "caller", "callers"),
]);

/// A template that takes `target`.
pub const TRACE_TARGET: Template<Text> = Template::new(&[Piece::Text("target: "), Piece::Value(0)]);

/// A template that takes (`count`, `minimum`, `maximum`).
pub const TRACE_TARGET_COUNT: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("target holds "),
    Piece::Value(0),
    Piece::Text(" targets, but trace takes "),
    Piece::Value(1),
    Piece::Text(" to "),
    Piece::Value(2),
    Piece::Text(" targets."),
]);

/// A template that takes `name`.
pub const TRACE_TIER_UNCHECKED: Template<Text> =
    Template::new(&[Piece::Value(0), Piece::Text(", locals not checked")]);

/// A template that takes `count`.
pub const TRACE_TREE_UNLISTED: Template<Count> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(0),
    Piece::Text(" more: trace a subfolder for the rest"),
]);

/// A template that takes (`path`, `paths`).
pub const TRACE_UNKNOWN_PATH: Template<(Text, Text)> = Template::new(&[
    Piece::Text("no file or folder "),
    Piece::Value(0),
    Piece::Text(" in the repository. Paths that end the same way: "),
    Piece::Value(1),
    Piece::Text("."),
]);

/// A template that takes `path`.
pub const TRACE_UNKNOWN_PATH_NONE: Template<Text> = Template::new(&[
    Piece::Text("no file or folder "),
    Piece::Value(0),
    Piece::Text(" in the repository."),
]);

/// A template that takes (`check`, `reads`, `compared`, `budget`).
pub const TREE_CHECK_STOPPED: Template<(Text, Text, Text, Count)> = Template::new(&[
    Piece::Text("not checked: "),
    Piece::Value(0),
    Piece::Text(" stopped at its time budget of "),
    Piece::Value(3),
    Piece::Text(" s before it read "),
    Piece::Value(1),
    Piece::Text(" in the repository, so it compared "),
    Piece::Value(2),
    Piece::Text("."),
]);

/// A text that takes no values.
pub const TREE_CHECK_STOPPED_EVIDENCE: &str = "time budget spent";

/// A template that takes `listed`.
pub const UNKNOWN_TOOL: Template<Text> = Template::new(&[
    Piece::Text("No tool has this name. The tools are "),
    Piece::Value(0),
    Piece::Text("."),
]);

/// A template that takes (`root`, `bytes`).
pub const WALK_LISTING_CAPPED: Template<(Text, Count)> = Template::new(&[
    Piece::Text("walk refused: git listed more than "),
    Piece::Value(1),
    Piece::Text(" bytes of names under "),
    Piece::Value(0),
]);

/// A template that takes `root`.
pub const WALK_NOT_DIRECTORY: Template<Text> =
    Template::new(&[Piece::Text("not a directory: "), Piece::Value(0)]);

/// A template that takes `reason`.
pub const WALK_REFUSED_CLI: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        ". No flag changes a walk bound. Set --repo to a subdirectory, \
        and the walk covers only that subtree.",
    ),
]);

/// A template that takes `reason`.
pub const WALK_REFUSED_MCP: Template<Text> = Template::new(&[
    Piece::Value(0),
    Piece::Text(
        ". No argument changes a walk bound. Serve a subdirectory with \
        --root, and the walk covers only that subtree.",
    ),
]);

/// A template that takes (`root`, `count`, `limit`).
pub const WALK_TOO_DEEP: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("walk refused: directory depth "),
    Piece::Value(1),
    Piece::Text(" exceeds the limit of "),
    Piece::Value(2),
    Piece::Text(" under "),
    Piece::Value(0),
]);

/// A template that takes (`root`, `limit`).
pub const WALK_TOO_MANY_BYTES: Template<(Text, Count)> = Template::new(&[
    Piece::Text("walk refused: the files that a scan reads under "),
    Piece::Value(0),
    Piece::Text(" hold more than "),
    Piece::Value(1),
    Piece::Text(" bytes"),
]);

/// A template that takes (`root`, `limit`).
pub const WALK_TOO_MANY_FILES: Template<(Text, Count)> = Template::new(&[
    Piece::Text("walk refused: more than "),
    Piece::Value(1),
    Piece::Text(" files under "),
    Piece::Value(0),
]);

/// A template that takes (`root`, `note`).
pub const WALK_UNLISTED: Template<(Text, Text)> = Template::new(&[
    Piece::Value(0),
    Piece::Text(" cannot be listed the way its .gitignore asks: "),
    Piece::Value(1),
]);
