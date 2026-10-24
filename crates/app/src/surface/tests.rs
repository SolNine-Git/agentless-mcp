use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use json::JsonValue;
use text::testing::cases;

use super::{Calls, Operation, Presence, Shape, TOOLS, Tool, tool_entry};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "surface/surface";
// Every operation of a tool takes these, so no table row lists them.
const UNIVERSAL: [&str; 3] = ["operation", "repo_root", "no_cache"];

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
        let entry = tool_entry(tool);
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
