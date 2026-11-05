use std::sync::Arc;

use crate::{Language, NodeId, Parsed, TypeChecking, parse};

fn parsed(language: Language, source: &str) -> Parsed {
    parse(language, Arc::from(source)).expect("the sample parses")
}

// The condition text of each `if` statement, with whether the clone walk skips the statement.
fn skipped_ifs(parsed: &Parsed) -> Vec<(String, bool)> {
    let spec = parsed.language().spec().expect("a tier-1 language");
    let skipped = spec.clones.skipped(parsed);
    let condition = parsed
        .language()
        .field("condition")
        .expect("a condition field");
    let ifs: Vec<NodeId> = parsed
        .descendants(parsed.root())
        .filter(|node| parsed.kind_name(*node) == "if_statement")
        .collect();
    ifs.into_iter()
        .map(|node| {
            let test = parsed.child_by_field(node, condition).expect("a condition");
            (parsed.text(test).to_owned(), skipped.contains(node))
        })
        .collect()
}

#[test]
fn type_checking_holds_the_fixed_spellings_and_each_name_the_file_binds_to_them() {
    let source = "import typing as t, os as o\nfrom typing import TYPE_CHECKING as CHECKING, Any as A\n\
                  import typing.io as tio\nimport typing_extensions as ext\n\
                  from .typing import TYPE_CHECKING as LOCAL\n\n\
                  def f():\n    from typing import TYPE_CHECKING as INNER\n";
    let found = TypeChecking::of(&parsed(Language::Python, source));
    let conditions = [
        ("TYPE_CHECKING", true),
        ("typing.TYPE_CHECKING", true),
        ("t.TYPE_CHECKING", true),
        ("CHECKING", true),
        ("INNER", true),
        ("t", false),
        ("o.TYPE_CHECKING", false),
        ("A", false),
        ("tio.TYPE_CHECKING", false),
        ("ext.TYPE_CHECKING", false),
        ("LOCAL", false),
        ("not TYPE_CHECKING", false),
        ("t.TYPE_CHECKING and x", false),
    ];
    for (condition, expected) in conditions {
        assert_eq!(found.tests(condition), expected, "{condition}");
    }
}

#[test]
fn a_file_without_an_alias_tests_only_the_fixed_spellings() {
    let found = TypeChecking::of(&parsed(Language::Python, "import typing\n"));
    assert_eq!(found, TypeChecking::default());
    assert!(found.tests("typing.TYPE_CHECKING"));
    assert!(!found.tests("t.TYPE_CHECKING"));
}

#[test]
fn the_clone_walk_skips_a_block_under_any_spelling_that_tests_type_checking() {
    let source = "import typing as t\nimport os as o\nfrom typing import TYPE_CHECKING as CHECKING\n\n\
                  if t.TYPE_CHECKING:\n    pass\n\nif CHECKING:\n    pass\n\n\
                  if TYPE_CHECKING:\n    pass\n\nif o.TYPE_CHECKING:\n    pass\n\nif x:\n    pass\n";
    assert_eq!(
        skipped_ifs(&parsed(Language::Python, source)),
        [
            ("t.TYPE_CHECKING".to_owned(), true),
            ("CHECKING".to_owned(), true),
            ("TYPE_CHECKING".to_owned(), true),
            ("o.TYPE_CHECKING".to_owned(), false),
            ("x".to_owned(), false),
        ]
    );
}

#[test]
fn a_language_without_the_skip_rule_skips_nothing() {
    let source = "import * as t from 'typing';\nif (t.TYPE_CHECKING) {\n  run();\n}\n";
    assert_eq!(
        skipped_ifs(&parsed(Language::Javascript, source)),
        [("(t.TYPE_CHECKING)".to_owned(), false)]
    );
}
