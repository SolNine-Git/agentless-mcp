//! The Rust roles. A pattern binding and each use of it in scope are no reference, a member name
//! and the last segment of a type path bind nothing, and the name of an item that is no symbol
//! declares.

use std::sync::Arc;

use facts::Role::{Attribute, Binding, Declaration, Local, Reference};
use facts::{Role, extract};
use syntax::{Language, parse};

// The line and the role of each occurrence of `name`, in document order.
fn roles(source: &str, name: &str) -> Vec<(u32, Role)> {
    let parsed = parse(Language::Rust, Arc::from(source)).expect("the source parses");
    let facts = extract(&parsed);
    let named = facts.refs.iter().filter(|reference| reference.name == name);
    named
        .map(|reference| (reference.line, reference.role))
        .collect()
}

#[test]
fn a_parameter_binds_and_each_use_in_the_body_is_a_local() {
    let source = "fn integer(record: &Pairs) -> i64 {\n    get(record)\n}\n";
    assert_eq!(roles(source, "record"), [(1, Binding), (2, Local)]);
}

#[test]
fn a_closure_parameter_is_a_local_in_the_closure_body_only() {
    let source = "fn count(lines: &[u8]) -> usize {\n    let n = lines.iter().map(|line| line \
                  + 1).count();\n    line\n}\n";
    assert_eq!(
        roles(source, "line"),
        [(2, Binding), (2, Local), (3, Reference)]
    );
}

#[test]
fn a_let_initializer_sees_the_names_before_the_let_and_not_its_own() {
    let unbound = "fn f() -> u8 {\n    let x = x;\n    x\n}\n";
    assert_eq!(
        roles(unbound, "x"),
        [(2, Binding), (2, Reference), (3, Local)]
    );
    let shadowed = "fn f(x: u8) -> u8 {\n    let x = x;\n    x\n}\n";
    assert_eq!(
        roles(shadowed, "x"),
        [(1, Binding), (2, Binding), (2, Local), (3, Local)]
    );
}

#[test]
fn a_call_in_the_initializer_of_a_let_of_its_name_stays_a_reference() {
    let source = "fn f() {\n    let tree = tree();\n    tree.len();\n}\n";
    assert_eq!(
        roles(source, "tree"),
        [(2, Binding), (2, Reference), (3, Local)]
    );
}

#[test]
fn the_else_block_of_a_let_else_does_not_see_its_names() {
    let source = "fn f(o: Option<u8>) -> u8 {\n    let Some(v) = o else {\n        return v;\n    \
                  };\n    v\n}\n";
    assert_eq!(
        roles(source, "v"),
        [(2, Binding), (3, Reference), (5, Local)]
    );
}

#[test]
fn an_if_let_name_is_a_local_in_the_then_block_and_not_in_the_else() {
    let source = "fn f(o: Option<u8>) -> u8 {\n    if let Some(x) = o {\n        x\n    } else \
                  {\n        x\n    }\n}\n";
    assert_eq!(
        roles(source, "x"),
        [(2, Binding), (3, Local), (5, Reference)]
    );
}

#[test]
fn a_while_let_name_is_a_local_in_the_loop_and_not_after_it() {
    let source = "fn f(mut it: I) -> u8 {\n    while let Some(item) = it.next() {\n        \
                  take(item);\n    }\n    item\n}\n";
    assert_eq!(
        roles(source, "item"),
        [(2, Binding), (3, Local), (5, Reference)]
    );
}

#[test]
fn a_let_chain_name_is_a_local_in_the_later_conditions_and_the_block() {
    let source = "fn f(p: P) -> u8 {\n    if let Some(a) = p.first() && let Some(b) = a.second() \
                  {\n        b\n    } else {\n        0\n    }\n}\n";
    assert_eq!(roles(source, "a"), [(2, Binding), (2, Local)]);
    assert_eq!(roles(source, "b"), [(2, Binding), (3, Local)]);
}

#[test]
fn a_match_arm_name_is_a_local_in_its_guard_and_its_value_only() {
    let source = "fn f(v: Option<u8>) -> u8 {\n    match v {\n        Some(x) if x \
                  > 0 => x,\n        _ => x,\n    }\n}\n";
    assert_eq!(
        roles(source, "x"),
        [(3, Binding), (3, Local), (3, Local), (4, Reference)]
    );
}

#[test]
fn each_name_of_a_for_pattern_is_a_local_in_the_body() {
    let source = "fn f(items: &[u8]) {\n    for (i, x) in items.iter().enumerate() {\n        \
                  both(i, x);\n    }\n}\n";
    assert_eq!(roles(source, "i"), [(2, Binding), (3, Local)]);
    assert_eq!(roles(source, "x"), [(2, Binding), (3, Local)]);
}

#[test]
fn a_nested_function_does_not_see_the_outer_locals_and_a_closure_does() {
    let nested = "fn outer(record: u8) {\n    fn inner() -> u8 {\n        record\n    }\n}\n";
    assert_eq!(roles(nested, "record"), [(1, Binding), (3, Reference)]);
    let closure = "fn outer(record: u8) {\n    let get = || record;\n}\n";
    assert_eq!(roles(closure, "record"), [(1, Binding), (2, Local)]);
}

#[test]
fn a_shorthand_field_initializer_uses_the_local_of_its_name() {
    let source = "fn new(name: String) -> Item {\n    Item { name }\n}\n";
    assert_eq!(roles(source, "name"), [(1, Binding), (2, Local)]);
}

#[test]
fn an_upper_case_pattern_names_a_constant_and_stays_a_reference() {
    let source = "fn f(v: u8) -> u8 {\n    match v {\n        MAX => 1,\n        max => max,\n    \
                  }\n}\n";
    assert_eq!(roles(source, "MAX"), [(3, Reference)]);
    assert_eq!(roles(source, "max"), [(4, Binding), (4, Local)]);
}

#[test]
fn a_macro_argument_names_a_local_a_member_or_a_path() {
    let source = "fn f(record: &R, a: &A) {\n    dbg!(record);\n    assert!(a.flag);\n    let v = \
                  vec![m::make()];\n}\n";
    assert_eq!(roles(source, "record"), [(1, Binding), (2, Local)]);
    assert_eq!(roles(source, "a"), [(1, Binding), (3, Local)]);
    assert_eq!(roles(source, "flag"), [(3, Attribute)]);
    assert_eq!(roles(source, "m"), [(4, Reference)]);
    assert_eq!(roles(source, "make"), [(4, Reference)]);
}

#[test]
fn a_macro_rules_body_keeps_its_tokens_and_its_name_declares() {
    let source = "macro_rules! call {\n    ($x:expr) => {\n        record($x)\n    };\n}\n\nfn \
                  f(record: u8) {}\n";
    assert_eq!(roles(source, "record"), [(3, Reference), (7, Binding)]);
    assert_eq!(roles(source, "call"), [(1, Declaration)]);
}

#[test]
fn a_member_name_and_the_last_segment_of_a_type_path_bind_nothing() {
    let source = "fn g(x: X) {\n    x.f;\n    x.f();\n    x.f::<T>();\n    T::f();\n    \
                  Self::f;\n    <T>::f();\n    Enum::Variant;\n}\n";
    let tails: Vec<(u32, Role)> = (2..=7).map(|line| (line, Attribute)).collect();
    assert_eq!(roles(source, "f"), tails);
    assert_eq!(roles(source, "Variant"), [(8, Attribute)]);
    assert_eq!(
        roles(source, "T"),
        [(4, Reference), (5, Reference), (7, Reference)]
    );
    assert_eq!(roles(source, "Self"), [(6, Reference)]);
    assert_eq!(roles(source, "Enum"), [(8, Reference)]);
}

#[test]
fn a_field_name_binds_nothing_where_it_is_declared_initialised_or_matched() {
    let source = "struct S {\n    f: u8,\n}\n\nfn g(s: S) -> S {\n    let S { f: v } = s;\n    S { \
                  f: v }\n}\n";
    assert_eq!(
        roles(source, "f"),
        [(2, Attribute), (6, Attribute), (7, Attribute)]
    );
}

#[test]
fn the_name_of_an_item_that_is_no_symbol_declares() {
    let source = "enum Status {\n    Draft,\n}\n\nmod internal {}\n\nfn helper() {\n    fn \
                  nested() {}\n}\n";
    assert_eq!(roles(source, "Draft"), [(2, Declaration)]);
    assert_eq!(roles(source, "internal"), [(5, Declaration)]);
    assert_eq!(roles(source, "nested"), [(8, Declaration)]);
    assert_eq!(roles(source, "helper"), [(7, Reference)]);
    assert_eq!(roles(source, "Status"), [(1, Reference)]);
}

#[test]
fn a_signature_parameter_is_a_binding_that_no_body_sees() {
    let source = "trait Store {\n    fn get(&self, key: u8) -> u8;\n}\n";
    assert_eq!(roles(source, "key"), [(2, Binding)]);
    assert_eq!(roles(source, "get"), [(2, Declaration)]);
}
