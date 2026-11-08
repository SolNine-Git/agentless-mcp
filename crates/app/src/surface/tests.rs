use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use json::JsonValue;

use super::{MORE_SECTIONS, Presence, RootCount, Shape, TOOLS, tool_entry};
use crate::trace::Section;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

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
fn no_tool_has_an_operation_table_and_the_tool_names_are_distinct() {
    let names: BTreeSet<&str> = TOOLS.iter().map(|tool| tool.name).collect();
    assert_eq!(names.len(), TOOLS.len());
    for tool in TOOLS {
        for param in tool.params {
            assert_ne!(param.name, "operation", "{}", tool.name);
            if param.presence == Presence::DefaultFalse {
                assert_eq!(param.shape, Shape::Bool, "{}.{}", tool.name, param.name);
            }
        }
    }
}

#[test]
fn the_more_values_are_the_trace_sections_in_order() {
    assert_eq!(MORE_SECTIONS, Section::ALL.map(Section::as_str));
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
fn the_target_schema_takes_one_name_or_a_list_of_one_to_eight() {
    let trace = TOOLS.iter().find(|tool| tool.name == "trace");
    let entry = tool_entry(trace.expect("the trace tool"), RootCount::One);
    let target = member(
        member(member(&entry, "inputSchema"), "properties"),
        "target",
    );
    let list = r#"{"items": {"type": "string"}, "maxItems": 8, "minItems": 1, "type": "array"}"#;
    let want = json::parse(&format!(r#"[{{"type": "string"}}, {list}]"#)).expect("a JSON schema");
    assert!(
        *member(target, "anyOf") == want,
        "{}",
        json::value_text(target)
    );
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
