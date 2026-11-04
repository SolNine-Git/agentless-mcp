use std::fmt::Write as _;
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use text::testing::document;
use tree_sitter::Node;

use crate::{FieldCursor, FieldId, Language, NodeId, Parsed, parse};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    language: String,
    source: String,
    nodes: u32,
    first_error: i64,
    digest: String,
}

fn cases() -> Vec<Case> {
    document(env!("CARGO_MANIFEST_DIR"), "parse")
}

fn parsed(case: &Case) -> Parsed {
    let language = Language::from_name(&case.language)
        .unwrap_or_else(|| panic!("{}: unknown language {}", case.name, case.language));
    parse(language, Arc::from(case.source.as_str()))
        .unwrap_or_else(|error| panic!("{}: {error}", case.name))
}

fn field_name(parsed: &Parsed, field: Option<FieldId>) -> &'static str {
    field
        .and_then(|field| parsed.tree.language().field_name_for_id(field.0.get()))
        .unwrap_or_default()
}

fn row(parsed: &Parsed, node: NodeId) -> String {
    let flags = [
        parsed.is_named(node),
        parsed.is_missing(node),
        parsed.is_error(node),
        parsed.has_error(node),
    ];
    let bits: String = flags
        .iter()
        .map(|flag| if *flag { '1' } else { '0' })
        .collect();
    format!(
        "{}|{}|{}|{}|{bits}",
        parsed.kind_name(node),
        parsed.start_byte(node),
        parsed.end_byte(node),
        field_name(parsed, parsed.field_of(node)),
    )
}

fn digest(parsed: &Parsed) -> String {
    let rows: Vec<String> = parsed
        .descendants(parsed.root())
        .map(|node| row(parsed, node))
        .collect();
    hex(&Sha256::digest(rows.join("\n").as_bytes()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").expect("a String takes every write");
        text
    })
}

#[test]
fn every_fixture_parses_as_the_python_bundle_parses_it() {
    let cases = cases();
    assert!(cases.len() > 400, "the fixture holds {} cases", cases.len());
    for case in &cases {
        let parsed = parsed(case);
        assert_eq!(parsed.node_count(), case.nodes, "{}: node count", case.name);
        assert_eq!(digest(&parsed), case.digest, "{}: node rows", case.name);
        let first_error = parsed.first_error().map_or(-1, |node| i64::from(node.0));
        assert_eq!(first_error, case.first_error, "{}: first error", case.name);
    }
}

#[test]
fn every_language_has_a_fixture_case() {
    let cases = cases();
    for language in Language::ALL {
        assert!(
            cases.iter().any(|case| case.language == language.name()),
            "no fixture case parses {language:?}"
        );
    }
}

#[test]
fn the_flat_table_equals_the_node_api_at_every_node() {
    for case in &cases() {
        let parsed = parsed(case);
        let mut stack = vec![(parsed.tree.root_node(), parsed.root())];
        while let Some((node, id)) = stack.pop() {
            assert_node(&parsed, node, id, &case.name);
            assert_fields(&parsed, node, id, &case.name);
            let children: Vec<NodeId> = parsed.children(id).collect();
            assert_eq!(children.len(), node.child_count(), "{}: {id:?}", case.name);
            for (index, child) in children.into_iter().enumerate() {
                let index = u32::try_from(index).expect("a child index fits u32");
                assert_eq!(parsed.parent(child), Some(id), "{}: {child:?}", case.name);
                stack.push((node.child(index).expect("the node has the child"), child));
            }
        }
    }
}

#[test]
fn the_previous_sibling_equals_a_forward_scan_of_the_parents_children_at_every_node() {
    let mut languages = std::collections::BTreeSet::new();
    for case in &cases() {
        let parsed = parsed(case);
        assert_eq!(parsed.prev_sibling(parsed.root()), None, "{}", case.name);
        for node in parsed.descendants(parsed.root()) {
            let Some(parent) = parsed.parent(node) else {
                continue;
            };
            let scanned = parsed
                .children(parent)
                .take_while(|child| *child != node)
                .last();
            assert_eq!(
                parsed.prev_sibling(node),
                scanned,
                "{}: {node:?}",
                case.name
            );
        }
        languages.insert(case.language.clone());
    }
    assert!(languages.len() >= 3, "{languages:?}");
}

#[test]
fn the_previous_sibling_of_a_first_child_or_the_root_is_none() {
    let source = "def f():\n    return 1\n\nx = 2\n";
    let parsed = parse(Language::Python, Arc::from(source)).expect("the source parses");
    let root = parsed.root();
    let statements: Vec<NodeId> = parsed.children(root).collect();
    let [function, statement] = statements[..] else {
        panic!("two statements: {statements:?}");
    };
    assert_eq!(parsed.prev_sibling(root), None);
    assert_eq!(parsed.prev_sibling(function), None);
    assert!(parsed.descendants(function).count() > 1);
    assert_eq!(parsed.prev_sibling(statement), Some(function));
    assert_eq!(parsed.kind_name(statement), "assignment");
    let parts: Vec<NodeId> = parsed.children(statement).collect();
    let [name, equals, value] = parts[..] else {
        panic!("three parts: {parts:?}");
    };
    assert!(parsed.is_leaf(name) && parsed.is_leaf(equals));
    assert_eq!(parsed.prev_sibling(equals), Some(name));
    assert_eq!(parsed.prev_sibling(value), Some(equals));
    assert_eq!(parsed.prev_sibling(name), None);
}

#[test]
fn the_next_sibling_equals_a_forward_scan_of_the_parents_children_at_every_node() {
    let mut languages = std::collections::BTreeSet::new();
    for case in &cases() {
        let parsed = parsed(case);
        assert_eq!(parsed.next_sibling(parsed.root()), None, "{}", case.name);
        for node in parsed.descendants(parsed.root()) {
            let Some(parent) = parsed.parent(node) else {
                continue;
            };
            let scanned = parsed
                .children(parent)
                .skip_while(|child| *child != node)
                .nth(1);
            assert_eq!(
                parsed.next_sibling(node),
                scanned,
                "{}: {node:?}",
                case.name
            );
        }
        languages.insert(case.language.clone());
    }
    assert!(languages.len() >= 3, "{languages:?}");
}

#[test]
fn the_next_sibling_of_a_last_child_or_the_root_is_none() {
    let source = "def f():\n    return 1\n\nx = 2\n";
    let parsed = parse(Language::Python, Arc::from(source)).expect("the source parses");
    let root = parsed.root();
    let statements: Vec<NodeId> = parsed.children(root).collect();
    let [function, statement] = statements[..] else {
        panic!("two statements: {statements:?}");
    };
    assert_eq!(parsed.next_sibling(root), None);
    assert!(parsed.descendants(function).count() > 1);
    assert_eq!(parsed.next_sibling(function), Some(statement));
    assert_eq!(parsed.next_sibling(statement), None);
    let parts: Vec<NodeId> = parsed.children(statement).collect();
    let [name, equals, value] = parts[..] else {
        panic!("three parts: {parts:?}");
    };
    assert!(parsed.is_leaf(name) && parsed.is_leaf(equals));
    assert_eq!(parsed.next_sibling(name), Some(equals));
    assert_eq!(parsed.next_sibling(equals), Some(value));
    assert_eq!(parsed.next_sibling(value), None);
}

fn assert_node(parsed: &Parsed, node: Node, id: NodeId, name: &str) {
    let at = format!("{name}: {id:?}");
    assert_eq!(parsed.kind_name(id), node.kind(), "{at}");
    let named = parsed.language().kinds(&[node.kind()]);
    assert!(named.contains(parsed.kind(id)), "{at}");
    assert_eq!(parsed.is_named(id), node.is_named(), "{at}");
    assert_eq!(parsed.is_missing(id), node.is_missing(), "{at}");
    assert_eq!(parsed.is_error(id), node.is_error(), "{at}");
    assert_eq!(parsed.has_error(id), node.has_error(), "{at}");
    assert_eq!(parsed.byte_range(id), node.byte_range(), "{at}");
    let start = parsed.start_point(id);
    let end = parsed.end_point(id);
    let (expected_start, expected_end) = (node.start_position(), node.end_position());
    assert_eq!(
        (start.row as usize, start.column as usize),
        (expected_start.row, expected_start.column),
        "{at}"
    );
    assert_eq!(
        (end.row as usize, end.column as usize),
        (expected_end.row, expected_end.column),
        "{at}"
    );
    assert_eq!(parsed.child_count(id), node.child_count(), "{at}");
    assert_eq!(
        parsed.named_child_count(id),
        node.named_child_count(),
        "{at}"
    );
    assert_eq!(parsed.is_leaf(id), node.child_count() == 0, "{at}");
    assert_eq!(
        parsed.descendants(id).count(),
        node.descendant_count(),
        "{at}"
    );
}

fn assert_fields(parsed: &Parsed, node: Node, id: NodeId, name: &str) {
    let grammar = parsed.tree.language();
    let count = u16::try_from(grammar.field_count()).expect("a field count fits u16");
    for raw in 1..=count {
        let field = FieldId(core::num::NonZeroU16::new(raw).expect("field ids start at 1"));
        let at = format!("{name}: {id:?} field {raw}");
        let mut cursor = node.walk();
        let expected: Vec<usize> = node
            .children_by_field_id(field.0, &mut cursor)
            .map(|child| child.id())
            .collect();
        let found: Vec<usize> = parsed
            .children_by_field(id, field)
            .map(|child| node_at(parsed, child).id())
            .collect();
        assert_eq!(found, expected, "{at}: children by field");
        let expected = node.child_by_field_id(raw).map(|child| child.id());
        let found = parsed
            .child_by_field(id, field)
            .map(|child| node_at(parsed, child).id());
        assert_eq!(found, expected, "{at}: child by field");
    }
}

// Every field of the grammar, so every field that a facts walker reads.
#[test]
fn a_field_cursor_answers_as_child_by_field_in_document_and_reverse_order() {
    for case in &cases() {
        let parsed = parsed(case);
        let count = u16::try_from(parsed.tree.language().field_count()).expect("fits u16");
        let fields: Vec<FieldId> = (1..=count)
            .filter_map(|raw| core::num::NonZeroU16::new(raw).map(FieldId))
            .collect();
        let lookups: Vec<(NodeId, FieldId, Option<NodeId>)> = parsed
            .descendants(parsed.root())
            .flat_map(|node| fields.iter().map(move |field| (node, *field)))
            .map(|(node, field)| (node, field, parsed.child_by_field(node, field)))
            .collect();
        assert_cursor(&parsed, lookups.iter(), &case.name);
        let reverse = format!("{} in reverse", case.name);
        assert_cursor(&parsed, lookups.iter().rev(), &reverse);
    }
}

fn assert_cursor<'a>(
    parsed: &Parsed,
    lookups: impl Iterator<Item = &'a (NodeId, FieldId, Option<NodeId>)>,
    name: &str,
) {
    let mut cursor = FieldCursor::new(parsed);
    for (node, field, expected) in lookups {
        let found = cursor.child_by_field(*node, *field);
        assert_eq!(found, *expected, "{name}: {node:?} {field:?}");
    }
}

fn node_at(parsed: &Parsed, id: NodeId) -> Node<'_> {
    let mut cursor = parsed.tree.walk();
    cursor.goto_descendant(id.0 as usize);
    cursor.node()
}

#[test]
fn first_of_kind_finds_the_first_node_in_document_order() {
    let source = "def f():\n    return g(x, y)\n";
    let parsed = parse(Language::Python, Arc::from(source)).expect("the source parses");
    let identifiers = Language::Python.kinds(&["identifier"]);
    let first = parsed
        .first_of_kind(parsed.root(), &identifiers)
        .expect("an identifier");
    assert_eq!(parsed.text(first), "f");
    let call = parsed
        .first_of_kind(parsed.root(), &Language::Python.kinds(&["call"]))
        .expect("a call");
    let inner = parsed
        .first_of_kind(call, &identifiers)
        .expect("an identifier in the call");
    assert_eq!(parsed.text(inner), "g");
    assert_eq!(
        parsed.first_of_kind(call, &Language::Python.kinds(&["class_definition"])),
        None
    );
}

#[test]
fn points_count_rows_by_line_feed_and_columns_by_byte() {
    let source = "a = \"\u{e9}\"\r\nb = 1\n";
    let parsed = parse(Language::Python, Arc::from(source)).expect("the source parses");
    let names = Language::Python.kinds(&["identifier"]);
    let second = parsed
        .descendants(parsed.root())
        .filter(|node| names.contains(parsed.kind(*node)))
        .nth(1)
        .expect("two identifiers");
    assert_eq!(parsed.text(second), "b");
    assert_eq!(
        parsed.start_point(second),
        crate::Point { row: 1, column: 0 }
    );
    assert_eq!(parsed.end_point(parsed.root()).row, 2);
}

#[test]
fn a_parse_is_send_and_sync() {
    fn shared<T: Send + Sync>() {}
    shared::<Parsed>();
}

#[test]
fn a_kind_name_at_another_address_resolves_by_name() {
    let kinds = crate::kinds::table(Language::Python);
    let copy: &'static str = String::from("identifier").leak();
    assert_eq!(kinds.id(copy), kinds.id_by_name("identifier"));
    assert!(kinds.id(copy).is_some());
    assert_eq!(kinds.id(String::from("no such kind").leak()), None);
}
