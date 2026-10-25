use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use json::JsonValue;
use text::CodePointString;
use text::testing::cases;

use super::{Calls, Operation, Presence, RootCount, Shape, TOOLS, Tool, tool_entry};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "surface/surface";
// Every operation of a tool takes these, so no table row lists them.
const UNIVERSAL: [&str; 2] = ["operation", "repo_root"];

// The tools section, read with its key order and its number forms kept.
fn reference_tools() -> Vec<JsonValue> {
    let path = Path::new(MANIFEST_DIR).join("fixtures/surface/surface.json");
    let shown = path.display();
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let document = json::parse(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let JsonValue::Object(sections) = &document else {
        panic!("{shown}: the document is not an object");
    };
    sections
        .iter()
        .find_map(|(name, value)| match value {
            JsonValue::Array(tools) if name.as_str() == Some("tools") => Some(tools.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{shown}: no tools section"))
}

fn operations(tool: &Tool) -> &'static [Operation] {
    match tool.calls {
        Calls::Single { .. } => &[],
        Calls::Operations(operations) => operations,
    }
}

#[test]
fn each_tool_entry_is_the_reference_entry() {
    let reference = reference_tools();
    assert_eq!(reference.len(), TOOLS.len());
    for (tool, expected) in TOOLS.iter().zip(&reference) {
        let entry = tool_entry(tool, RootCount::One);
        assert!(
            entry == *expected,
            "{}:\n{}\n{}",
            tool.name,
            json::value_text(&entry),
            json::value_text(expected)
        );
    }
}

#[test]
fn the_operation_tables_are_the_reference_tables() {
    let reference: Vec<(String, String, Vec<String>, Vec<String>)> =
        cases(MANIFEST_DIR, FIXTURE, "operations");
    let table: Vec<(&str, &Operation)> = TOOLS
        .iter()
        .flat_map(|tool| {
            operations(tool)
                .iter()
                .map(|operation| (tool.name, operation))
        })
        .collect();
    assert_eq!(table.len(), reference.len());
    for ((tool, operation), (name, row, accepted, required)) in table.iter().zip(&reference) {
        assert_eq!((*tool, operation.name), (name.as_str(), row.as_str()));
        assert_eq!(operation.accepted, &accepted[..], "{name}.{row}");
        assert_eq!(operation.required, &required[..], "{name}.{row}");
    }
}

#[test]
fn localizes_follows_the_reference_unlock_rule() {
    let reference: Vec<(String, Option<String>, Option<String>)> =
        cases(MANIFEST_DIR, FIXTURE, "unlock");
    assert!(!reference.is_empty());
    for (name, operation, reason) in reference {
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let localizes = tool.localizes(operation.as_deref());
        assert_eq!(localizes, reason.is_some(), "{name} {operation:?}");
    }
}

#[test]
fn each_operation_names_only_parameters_of_its_tool() {
    for tool in TOOLS.iter().filter(|tool| !operations(tool).is_empty()) {
        let declared: BTreeSet<&str> = tool.params.iter().map(|param| param.name).collect();
        let accepted: BTreeSet<&str> = operations(tool)
            .iter()
            .flat_map(|operation| operation.accepted.iter().copied())
            .collect();
        assert!(accepted.is_subset(&declared), "{}", tool.name);
        let rest: Vec<&str> = declared.difference(&accepted).copied().collect();
        assert!(
            rest.iter().all(|name| UNIVERSAL.contains(name)),
            "{}: {rest:?}",
            tool.name
        );
        for operation in operations(tool) {
            assert!(
                operation
                    .required
                    .iter()
                    .all(|name| operation.accepted.contains(name)),
                "{}.{}",
                tool.name,
                operation.name
            );
        }
    }
}

#[test]
fn a_tool_selects_by_operation_exactly_when_it_has_a_table() {
    for tool in TOOLS {
        let selects = tool.params.iter().any(|param| param.name == "operation");
        assert_eq!(selects, !operations(tool).is_empty(), "{}", tool.name);
        for param in tool.params {
            if param.presence == Presence::DefaultFalse {
                assert_eq!(param.shape, Shape::Bool, "{}.{}", tool.name, param.name);
            }
        }
    }
}

#[test]
fn operation_finds_each_row_and_the_tool_names_are_distinct() {
    let names: BTreeSet<&str> = TOOLS.iter().map(|tool| tool.name).collect();
    assert_eq!(names.len(), TOOLS.len());
    for tool in TOOLS {
        for operation in operations(tool) {
            let found = tool.operation(operation.name);
            assert!(found.is_some_and(|found| std::ptr::eq(found, operation)));
        }
        assert!(tool.operation("no_such_operation").is_none());
    }
}

// A refusal names the one tool that has an operation, so no two tools share an operation name.
#[test]
fn each_operation_name_belongs_to_one_tool() {
    let names: Vec<&str> = TOOLS
        .iter()
        .flat_map(|tool| operations(tool).iter().map(|operation| operation.name))
        .collect();
    let distinct: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(distinct.len(), names.len(), "{names:?}");
}

// With several roots each schema lists `repo_root` after the parameter that a call gives first,
// or alone, and nothing else in the entry changes.
#[test]
fn several_roots_add_repo_root_and_change_nothing_else() {
    for tool in TOOLS {
        let one = tool_entry(tool, RootCount::One);
        let mut several = tool_entry(tool, RootCount::Several);
        let JsonValue::Object(params) =
            member_mut(member_mut(&mut several, "inputSchema"), "properties")
        else {
            panic!("{}: the properties are no object", tool.name);
        };
        let position = params
            .iter()
            .position(|(name, _)| name.as_str() == Some("repo_root"));
        assert_eq!(
            position,
            Some(usize::from(params.len() > 1)),
            "{}",
            tool.name
        );
        params.remove(position.unwrap_or_default());
        assert!(
            several == one,
            "{}:\n{}\n{}",
            tool.name,
            json::value_text(&several),
            json::value_text(&one)
        );
    }
}

fn member<'v>(value: &'v JsonValue, key: &str) -> &'v JsonValue {
    let JsonValue::Object(entries) = value else {
        panic!("no object holds {key}");
    };
    json::get(entries, key).unwrap_or_else(|| panic!("no key {key}"))
}

fn member_mut<'v>(value: &'v mut JsonValue, key: &str) -> &'v mut JsonValue {
    let JsonValue::Object(entries) = value else {
        panic!("no object holds {key}");
    };
    let found = entries
        .iter_mut()
        .find(|(name, _)| name.as_str() == Some(key));
    found.map_or_else(|| panic!("no key {key}"), |(_, value)| value)
}

#[test]
fn the_lines_schema_takes_one_pair_or_a_list_of_pairs() {
    let read = TOOLS.iter().find(|tool| tool.name == "read");
    let entry = tool_entry(read.expect("the read tool"), RootCount::One);
    let lines = member(member(member(&entry, "inputSchema"), "properties"), "lines");
    let pair = r#"{"items": {"minimum": 1, "type": "integer"}, "maxItems": 2, "minItems": 2, "type": "array"}"#;
    let choices = format!(r#"[{pair}, {{"items": {pair}, "minItems": 1, "type": "array"}}]"#);
    let want = json::parse(&choices).expect("a JSON schema");
    assert!(
        *member(lines, "anyOf") == want,
        "{}",
        json::value_text(lines)
    );
}

#[test]
fn each_operation_enum_is_the_operation_table() {
    for tool in TOOLS.iter().filter(|tool| !operations(tool).is_empty()) {
        let entry = tool_entry(tool, RootCount::One);
        let operation = member(
            member(member(&entry, "inputSchema"), "properties"),
            "operation",
        );
        let names = operations(tool)
            .iter()
            .map(|operation| JsonValue::Str(CodePointString::from(operation.name)));
        let want = JsonValue::Array(names.collect());
        assert!(
            *member(operation, "enum") == want,
            "{}: {}",
            tool.name,
            json::value_text(operation)
        );
    }
}

// An optional parameter publishes its own shape: no null choice and no null default.
#[test]
fn no_parameter_publishes_null() {
    let null = json::parse(r#"{"type": "null"}"#).expect("a JSON schema");
    for tool in TOOLS {
        let entry = tool_entry(tool, RootCount::Several);
        let JsonValue::Object(params) = member(member(&entry, "inputSchema"), "properties") else {
            panic!("{}: the properties are no object", tool.name);
        };
        for (name, schema) in params {
            let JsonValue::Object(keys) = schema else {
                panic!("{}.{name:?}: the schema is no object", tool.name);
            };
            let offers_null = match json::get(keys, "anyOf") {
                Some(JsonValue::Array(choices)) => choices.contains(&null),
                _ => false,
            };
            let null_default = json::get(keys, "default") == Some(&JsonValue::Null);
            assert!(!offers_null && !null_default, "{}.{name:?}", tool.name);
        }
    }
}
