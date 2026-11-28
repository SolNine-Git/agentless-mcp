use std::collections::BTreeSet;

use prompts::CommentGroup;
use serde::Deserialize;
use syntax::Language;

use super::{comment_delta, compile, rules_for};
use crate::{CheckGap, CommentState, Detail, Pair, PairFinding, Severity, SideText, TextPair};

// One recorded outcome of comment_rules.py: each fresh finding as its line, its key with the rule
// first, and its message.
#[derive(Deserialize)]
struct Case {
    name: String,
    path: String,
    old: Option<String>,
    new: String,
    findings: Vec<(u32, Vec<String>, String)>,
}

type Shown = (u32, String, String, String);

fn pairs(path: &str, old: Option<&str>, new: Option<&str>) -> Vec<Pair> {
    let text = TextPair {
        path: path.to_owned(),
        old: old.map(str::to_owned),
        new: new.map(str::to_owned),
    };
    Pair::measure_all(vec![text], |sides| {
        sides.iter().map(SideText::measure).collect()
    })
}

fn run(pairs: &[Pair]) -> crate::Outcome {
    let refs: Vec<&Pair> = pairs.iter().collect();
    comment_delta(&refs).expect("the run succeeds")
}

// The key parts join with a newline, so a block run and a line run compare the same way.
fn shown(row: &PairFinding) -> Shown {
    let Some(Detail::Comment(key)) = &row.detail else {
        panic!("{row:?} has no comment key");
    };
    let finding = &row.finding;
    assert_eq!(finding.severity, Severity::Warning, "{row:?}");
    let message = finding.message.clone();
    (finding.line, key.rule.clone(), key.key.clone(), message)
}

fn case(name: &str) {
    let cases: Vec<Case> = text::testing::cases(env!("CARGO_MANIFEST_DIR"), "comments", "cases");
    let case = cases
        .iter()
        .find(|case| case.name == name)
        .expect("a recorded case");
    let pairs = pairs(&case.path, case.old.as_deref(), Some(&case.new));
    let found: Vec<Shown> = run(&pairs).findings.iter().map(shown).collect();
    let expected: Vec<Shown> = (case.findings.iter())
        .map(|(line, key, message)| (*line, key[0].clone(), key[1..].join("\n"), message.clone()))
        .collect();
    assert_eq!(found, expected, "case {name}");
}

#[test]
fn python_suppression() {
    case("python_suppression");
}

#[test]
fn python_banner() {
    case("python_banner");
}

#[test]
fn python_run() {
    case("python_run");
}

#[test]
fn python_private_doc() {
    case("python_private_doc");
}

#[test]
fn python_section() {
    case("python_section");
}

#[test]
fn python_directive() {
    case("python_directive");
}

#[test]
fn python_banner_ends_run() {
    case("python_banner_ends_run");
}

#[test]
fn python_moved_comment() {
    case("python_moved_comment");
}

#[test]
fn python_duplicated_run() {
    case("python_duplicated_run");
}

#[test]
fn python_grown_run() {
    case("python_grown_run");
}

#[test]
fn python_syntax_error() {
    case("python_syntax_error");
}

#[test]
fn python_format_string_is_no_docstring() {
    case("python_format_string_is_no_docstring");
}

#[test]
fn python_joined_string_is_a_docstring() {
    case("python_joined_string_is_a_docstring");
}

#[test]
fn python_old_findings_stay() {
    case("python_old_findings_stay");
}

#[test]
fn javascript_suppression() {
    case("javascript_suppression");
}

#[test]
fn javascript_banner() {
    case("javascript_banner");
}

#[test]
fn javascript_section() {
    case("javascript_section");
}

#[test]
fn javascript_block_run() {
    case("javascript_block_run");
}

#[test]
fn javascript_line_run() {
    case("javascript_line_run");
}

#[test]
fn javascript_directive() {
    case("javascript_directive");
}

#[test]
fn typescript_suppression() {
    case("typescript_suppression");
}

#[test]
fn typescript_ts_ignore() {
    case("typescript_ts_ignore");
}

#[test]
fn typescript_banner() {
    case("typescript_banner");
}

#[test]
fn typescript_section() {
    case("typescript_section");
}

#[test]
fn typescript_block_run() {
    case("typescript_block_run");
}

#[test]
fn typescript_line_run() {
    case("typescript_line_run");
}

#[test]
fn typescript_directive() {
    case("typescript_directive");
}

#[test]
fn tsx_suppression() {
    case("tsx_suppression");
}

#[test]
fn tsx_banner() {
    case("tsx_banner");
}

#[test]
fn tsx_section() {
    case("tsx_section");
}

#[test]
fn tsx_block_run() {
    case("tsx_block_run");
}

#[test]
fn tsx_line_run() {
    case("tsx_line_run");
}

#[test]
fn tsx_directive() {
    case("tsx_directive");
}

#[test]
fn rust_suppression() {
    case("rust_suppression");
}

#[test]
fn rust_banner() {
    case("rust_banner");
}

#[test]
fn rust_line_run() {
    case("rust_line_run");
}

#[test]
fn rust_block_run() {
    case("rust_block_run");
}

#[test]
fn rust_private_doc() {
    case("rust_private_doc");
}

#[test]
fn rust_trait_impl_doc() {
    case("rust_trait_impl_doc");
}

#[test]
fn rust_section() {
    case("rust_section");
}

#[test]
fn rust_enum_variants() {
    case("rust_enum_variants");
}

#[test]
fn rust_trait_members() {
    case("rust_trait_members");
}

#[test]
fn rust_macro_body() {
    case("rust_macro_body");
}

#[test]
fn rust_macro_export() {
    case("rust_macro_export");
}

#[test]
fn rust_clap_parser() {
    case("rust_clap_parser");
}

#[test]
fn rust_raw_string() {
    case("rust_raw_string");
}

#[test]
fn rust_lifetime_char() {
    case("rust_lifetime_char");
}

#[test]
fn rust_nested_block() {
    case("rust_nested_block");
}

#[test]
fn rust_moved_attribute() {
    case("rust_moved_attribute");
}

#[test]
fn rust_facing() {
    case("rust_facing");
}

#[test]
fn every_recorded_case_has_its_own_test() {
    let cases: Vec<Case> = text::testing::cases(env!("CARGO_MANIFEST_DIR"), "comments", "cases");
    let source = include_str!("tests.rs");
    let untested: Vec<&str> = (cases.iter())
        .map(|case| case.name.as_str())
        .filter(|name| !source.contains(&format!("case(\"{name}\");")))
        .collect();
    assert!(untested.is_empty(), "{untested:?}");
}

// A side that does not parse cleanly is parsed again, so its comments still count as old.
#[test]
fn an_old_side_with_a_syntax_error_still_carries_its_comments() {
    let old = "# a\n# b\n# c\ndef f(:\n";
    let new = "# a\n# b\n# c\ndef f():\n    pass\n";
    let pairs = pairs("src/a.py", Some(old), Some(new));
    assert_eq!(run(&pairs).findings, []);
}

#[test]
fn a_language_with_no_rules_gives_one_not_checked_row_per_group() {
    let pairs = pairs("src/main.go", None, Some("// a\n// b\n// c\nfunc f() {}\n"));
    let outcome = run(&pairs);
    let severities: Vec<Severity> = (outcome.findings.iter())
        .map(|row| row.finding.severity)
        .collect();
    assert_eq!(severities, [Severity::NotChecked; 3]);
    let in_message_order = [
        CommentGroup::Docs,
        CommentGroup::Runs,
        CommentGroup::Suppressions,
    ];
    let expected: Vec<Detail> = in_message_order
        .map(|group| Detail::Gap(CheckGap::NoCommentRules(Language::Go, group)))
        .to_vec();
    let found: Vec<Detail> = (outcome.findings.iter())
        .filter_map(|row| row.detail.clone())
        .collect();
    assert_eq!(found, expected);
    assert_eq!(
        outcome.findings[0].finding.message,
        "not checked: comment_delta has no docs rules for go (doc_attachment), so it did not \
         check src/main.go for them."
    );
    assert_eq!(outcome.observed.comment_keys, None);
}

#[test]
fn a_deleted_file_gives_no_row() {
    for path in ["src/main.go", "src/a.py"] {
        let pairs = pairs(path, Some("# a\n"), None);
        assert_eq!(run(&pairs).findings, [], "{path}");
    }
}

#[test]
fn the_observed_keys_hold_every_finding_of_the_new_text() {
    let old = "# ----\nx = 1\n";
    let new = "# ----\nx = 1  # noqa\n";
    let pairs = pairs("src/a.py", Some(old), Some(new));
    let outcome = run(&pairs);
    let state = |rule: &str, key: &str| CommentState {
        path: "src/a.py".to_owned(),
        rule: rule.to_owned(),
        key: key.to_owned(),
    };
    let expected = vec![state("banner", "# ----"), state("suppress", "# noqa")];
    assert_eq!(outcome.observed.comment_keys, Some(expected));
    assert_eq!(outcome.findings.len(), 1);
}

// A new language file fails here until its row or its gap is in the rule file.
#[test]
fn every_language_file_has_rules_or_a_gap_in_each_group() {
    let files = crate::tests::language_files();
    let rows: BTreeSet<String> = (prompts::comment_rules().iter())
        .map(|rules| rules.language.to_owned())
        .collect();
    assert_eq!(rows, files);
    for language in Language::ALL {
        let rules = rules_for(language);
        let declared = CommentGroup::ALL.map(|group| rules.gap(group).is_some());
        let tier_one = language.spec().is_some();
        assert!(tier_one || declared.iter().all(|gap| *gap), "{rules:?}");
    }
}

// The rule table is static, so this test is the check that every pattern of it compiles.
#[test]
fn every_rule_pattern_compiles() {
    for language in Language::ALL {
        let compiled = compile(language, rules_for(language));
        let has_runs = rules_for(language).gap(CommentGroup::Runs).is_none();
        assert_eq!(compiled.is_some(), has_runs, "{language:?}");
    }
}

// The runs group of the five languages that gained rows: each comment finding of the new text.
// The test fails when the runs group is a gap, so the rows the task adds are what is tested.
fn findings(pairs: &[Pair]) -> Vec<Shown> {
    let outcome = run(pairs);
    assert!(
        !outcome.findings.iter().any(|row| matches!(
            row.detail,
            Some(Detail::Gap(CheckGap::NoCommentRules(_, CommentGroup::Runs)))
        )),
        "the runs group is a gap: {:?}",
        outcome.findings
    );
    outcome
        .findings
        .iter()
        .filter(|row| matches!(row.detail, Some(Detail::Comment(_))))
        .map(shown)
        .collect()
}

fn run_finding(line: u32, key: &str, message: &str) -> Shown {
    (line, "run".to_owned(), key.to_owned(), message.to_owned())
}

fn banner_finding(line: u32, key: &str) -> Shown {
    (
        line,
        "banner".to_owned(),
        key.to_owned(),
        "banner comment".to_owned(),
    )
}

#[test]
fn bash_line_run() {
    let pairs = pairs(
        "src/a.sh",
        Some("x=1\n"),
        Some("# one\n# two\n# three\nx=1\n"),
    );
    let expected = [run_finding(
        1,
        "# one\n# two\n# three",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn bash_banner() {
    let pairs = pairs("src/a.sh", Some("x=1\n"), Some("# ====\nx=1\n"));
    assert_eq!(findings(&pairs), [banner_finding(1, "# ====")]);
}

#[test]
fn bash_directive_is_exempt() {
    let pairs = pairs(
        "src/a.sh",
        Some("x=1\n"),
        Some("#!/bin/bash\n# shellcheck source=lib.sh\nx=1\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn bash_old_run_stays() {
    let old = "# one\n# two\n# three\nx=1\n";
    let pairs = pairs(
        "src/a.sh",
        Some(old),
        Some("# one\n# two\n# three\nx=1\ny=2\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn c_line_run() {
    let pairs = pairs(
        "src/a.c",
        Some("int main(void) { return 0; }\n"),
        Some("// one\n// two\n// three\nint main(void) { return 0; }\n"),
    );
    let expected = [run_finding(
        1,
        "// one\n// two\n// three",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn c_doc_run_is_exempt() {
    let pairs = pairs(
        "src/a.c",
        Some("int main(void) { return 0; }\n"),
        Some("/// one\n/// two\n/// three\nint main(void) { return 0; }\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn c_block_run() {
    let pairs = pairs(
        "src/a.c",
        Some("int main(void) { return 0; }\n"),
        Some("/* one\ntwo\nthree */\nint main(void) { return 0; }\n"),
    );
    let expected = [run_finding(
        1,
        "/* one\ntwo\nthree */",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn c_banner() {
    let pairs = pairs(
        "src/a.c",
        Some("int main(void) { return 0; }\n"),
        Some("// ====\nint main(void) { return 0; }\n"),
    );
    assert_eq!(findings(&pairs), [banner_finding(1, "// ====")]);
}

#[test]
fn c_directive_is_exempt() {
    let pairs = pairs(
        "src/a.c",
        Some("int main(void) { return 0; }\n"),
        Some("// clang-format off\nint main(void) { return 0; }\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn c_old_run_stays() {
    let old = "// one\n// two\n// three\nint main(void) { return 0; }\n";
    let pairs = pairs(
        "src/a.c",
        Some(old),
        Some("// one\n// two\n// three\nint main(void) { return 0; }\nint y = 1;\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn cpp_line_run() {
    let pairs = pairs(
        "src/a.cpp",
        Some("int main(void) { return 0; }\n"),
        Some("// one\n// two\n// three\nint main(void) { return 0; }\n"),
    );
    let expected = [run_finding(
        1,
        "// one\n// two\n// three",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn cpp_doc_run_is_exempt() {
    let pairs = pairs(
        "src/a.cpp",
        Some("int main(void) { return 0; }\n"),
        Some("/// one\n/// two\n/// three\nint main(void) { return 0; }\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn cpp_block_run() {
    let pairs = pairs(
        "src/a.cpp",
        Some("int main(void) { return 0; }\n"),
        Some("/* one\ntwo\nthree */\nint main(void) { return 0; }\n"),
    );
    let expected = [run_finding(
        1,
        "/* one\ntwo\nthree */",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn cpp_banner() {
    let pairs = pairs(
        "src/a.cpp",
        Some("int main(void) { return 0; }\n"),
        Some("// ====\nint main(void) { return 0; }\n"),
    );
    assert_eq!(findings(&pairs), [banner_finding(1, "// ====")]);
}

#[test]
fn cpp_directive_is_exempt() {
    let pairs = pairs(
        "src/a.cpp",
        Some("int main(void) { return 0; }\n"),
        Some("// clang-format on\nint main(void) { return 0; }\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn cpp_old_run_stays() {
    let old = "// one\n// two\n// three\nint main(void) { return 0; }\n";
    let pairs = pairs(
        "src/a.cpp",
        Some(old),
        Some("// one\n// two\n// three\nint main(void) { return 0; }\nint y = 1;\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn java_line_run() {
    let pairs = pairs(
        "src/a.java",
        Some("public class A {}\n"),
        Some("// one\n// two\n// three\npublic class A {}\n"),
    );
    let expected = [run_finding(
        1,
        "// one\n// two\n// three",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn java_doc_block_is_exempt() {
    let pairs = pairs(
        "src/a.java",
        Some("public class A {}\n"),
        Some("/** one\n * two\n * three */\npublic class A {}\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn java_block_run() {
    let pairs = pairs(
        "src/a.java",
        Some("public class A {}\n"),
        Some("/* one\ntwo\nthree */\npublic class A {}\n"),
    );
    let expected = [run_finding(
        1,
        "/* one\ntwo\nthree */",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn java_banner() {
    let pairs = pairs(
        "src/a.java",
        Some("public class A {}\n"),
        Some("// ====\npublic class A {}\n"),
    );
    assert_eq!(findings(&pairs), [banner_finding(1, "// ====")]);
}

#[test]
fn java_old_run_stays() {
    let old = "// one\n// two\n// three\npublic class A {}\n";
    let pairs = pairs(
        "src/a.java",
        Some(old),
        Some("// one\n// two\n// three\npublic class A {}\npublic class B {}\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn lua_line_run() {
    let pairs = pairs(
        "src/a.lua",
        Some("local x = 1\n"),
        Some("-- one\n-- two\n-- three\nlocal x = 1\n"),
    );
    let expected = [run_finding(
        1,
        "-- one\n-- two\n-- three",
        "3-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn lua_doc_run_is_exempt() {
    let pairs = pairs(
        "src/a.lua",
        Some("local x = 1\n"),
        Some("--- one\n--- two\n--- three\nlocal x = 1\n"),
    );
    assert_eq!(findings(&pairs), []);
}

#[test]
fn lua_block_run() {
    let pairs = pairs(
        "src/a.lua",
        Some("local x = 1\n"),
        Some("--[[\n-- one\n-- two\n-- three\n]]\nlocal x = 1\n"),
    );
    let expected = [run_finding(
        1,
        "--[[\n-- one\n-- two\n-- three\n]]",
        "5-line comment; why-comments run 1-2 lines",
    )];
    assert_eq!(findings(&pairs), expected);
}

#[test]
fn lua_banner() {
    let pairs = pairs(
        "src/a.lua",
        Some("local x = 1\n"),
        Some("-- ====\nlocal x = 1\n"),
    );
    assert_eq!(findings(&pairs), [banner_finding(1, "-- ====")]);
}

#[test]
fn lua_old_run_stays() {
    let old = "-- one\n-- two\n-- three\nlocal x = 1\n";
    let pairs = pairs(
        "src/a.lua",
        Some(old),
        Some("-- one\n-- two\n-- three\nlocal x = 1\nlocal y = 2\n"),
    );
    assert_eq!(findings(&pairs), []);
}
