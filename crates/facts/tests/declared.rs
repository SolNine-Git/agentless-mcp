//! The identifier that names a symbol where it is declared is no reference to it, in every
//! language. The resolver binds no spelling on a symbol's name line. A refs row drops a line only
//! when the line spells the name no more times than it declares it.

use std::sync::Arc;

use facts::{Declared, Facts, extract};
use syntax::{Language, parse};

fn facts_of(language: Language, source: &str) -> Facts {
    let parsed = parse(language, Arc::from(source)).expect("the source parses");
    extract(&parsed)
}

// Each identifier occurrence of `name` as (line, whether the resolver rule counts it declaring).
fn spellings(language: Language, source: &str, name: &str) -> Vec<(u32, bool)> {
    let facts = facts_of(language, source);
    let declared = Declared::of(&facts.symbols, &facts.refs);
    let named = facts.refs.iter().filter(|reference| reference.name == name);
    named
        .map(|reference| (reference.line, declared.declares(reference)))
        .collect()
}

// Each line that spells `name`, with whether the line only declares it.
fn lines(language: Language, source: &str, name: &str) -> Vec<(u32, bool)> {
    let facts = facts_of(language, source);
    let declared = Declared::of(&facts.symbols, &facts.refs);
    let mut lines: Vec<u32> = (facts.refs.iter())
        .filter(|reference| reference.name == name)
        .map(|reference| reference.line)
        .collect();
    lines.dedup();
    let verdicts = lines.into_iter();
    verdicts
        .map(|line| (line, declared.only_declares(name, line)))
        .collect()
}

#[test]
fn a_class_name_and_a_constructor_name_declare_and_a_use_references() {
    let source = "package x;\n\n@Entity\npublic class Deco {\n    public Deco() {\n    }\n\n    \
                  Deco copy() {\n        return new Deco();\n    }\n}\n";
    assert_eq!(
        spellings(Language::Java, source, "Deco"),
        [(4, true), (5, true), (8, false), (9, false)]
    );
    assert_eq!(
        lines(Language::Java, source, "Deco"),
        [(4, true), (5, true), (8, false), (9, false)]
    );
}

#[test]
fn a_typescript_function_name_declares_and_its_call_references() {
    let source = "export function helper(x: number): number {\n  return x;\n}\n\nhelper(1);\n";
    assert_eq!(
        spellings(Language::Typescript, source, "helper"),
        [(1, true), (5, false)]
    );
    assert_eq!(
        lines(Language::Typescript, source, "helper"),
        [(1, true), (5, false)]
    );
}

#[test]
fn a_python_definition_declares_by_its_role_and_its_name_line() {
    let source = "def helper():\n    return 1\n\n\nLIMIT = helper()\n";
    assert_eq!(
        spellings(Language::Python, source, "helper"),
        [(1, true), (5, false)]
    );
    assert_eq!(spellings(Language::Python, source, "LIMIT"), [(5, true)]);
    assert_eq!(lines(Language::Python, source, "LIMIT"), [(5, true)]);
}

#[test]
fn the_resolver_rule_drops_every_spelling_on_a_declaring_line() {
    let source = "class Holder {\n    static final int CJ = Other.CJ;\n}\n";
    assert_eq!(
        spellings(Language::Java, source, "CJ"),
        [(2, true), (2, true)]
    );
    let source = "from django.db import migrations\n\n\nclass \
                  Migration(migrations.Migration):\n    pass\n";
    assert_eq!(
        spellings(Language::Python, source, "Migration"),
        [(4, true), (4, true)]
    );
}

#[test]
fn a_line_that_spells_the_name_more_often_than_it_declares_it_holds_a_reference() {
    let source = "class Holder {\n    static final int CJ = Other.CJ;\n}\n";
    assert_eq!(lines(Language::Java, source, "CJ"), [(2, false)]);
    let source = "from django.db import migrations\n\n\nclass \
                  Migration(migrations.Migration):\n    pass\n";
    assert_eq!(lines(Language::Python, source, "Migration"), [(4, false)]);
}

#[test]
fn a_name_on_a_line_holds_only_where_a_symbol_of_that_name_is_named() {
    let facts = facts_of(Language::Python, "class A:\n    pass\n");
    let declared = Declared::of(&facts.symbols, &facts.refs);
    assert!(declared.holds("A", 1));
    assert!(!declared.holds("A", 2));
    assert!(!declared.holds("B", 1));
    assert!(!declared.only_declares("A", 2));
}
