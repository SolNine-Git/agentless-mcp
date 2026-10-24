use std::fs;
use std::path::Path;

use app::surface::{Shape, TOOLS};
use json::JsonValue;
use serde_json::Value as Json;
use text::CodePointString;

use super::coerce::tag;
use super::validate;
use crate::arguments::Value;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

// The cases of fixtures/validate.json: the arguments as serde_json reads them, as the server does,
// and the outcome read with its integer digits kept.
fn reference_cases() -> Vec<(Json, JsonValue)> {
    let path = Path::new(MANIFEST_DIR).join("fixtures/validate.json");
    let shown = path.display();
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let read: Json = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let kept = json::parse(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let cases = read["cases"].as_array().expect("a cases section").clone();
    let JsonValue::Object(sections) = &kept else {
        panic!("{shown}: no object");
    };
    let Some((_, JsonValue::Array(outcomes))) = sections.first() else {
        panic!("{shown}: no cases section");
    };
    let outcomes = outcomes.iter().map(|case| match case {
        JsonValue::Array(parts) => parts.last().expect("an outcome").clone(),
        _ => panic!("{shown}: a case is no list"),
    });
    cases.into_iter().zip(outcomes).collect()
}

fn outcome(name: &str, arguments: &Json) -> JsonValue {
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("no tool {name}"));
    match validate(tool, arguments) {
        Ok(values) => {
            let entries = values
                .iter()
                .map(|(name, value)| (key(name), value_json(value)));
            JsonValue::Object(vec![(key("ok"), JsonValue::Object(entries.collect()))])
        }
        Err(text) => JsonValue::Object(vec![(key("error"), JsonValue::Str(key(&text)))]),
    }
}

fn key(text: &str) -> CodePointString {
    CodePointString::from(text)
}

fn value_json(value: &Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Bool(flag) => JsonValue::Bool(*flag),
        Value::Int(number) => JsonValue::Int(number.to_string()),
        Value::Float(number) => JsonValue::Float(*number),
        Value::Text(text) => JsonValue::Str(key(text)),
        Value::List(items) => JsonValue::Array(items.iter().map(value_json).collect()),
    }
}

#[test]
fn each_case_validates_as_the_reference_says() {
    let cases = reference_cases();
    assert!(cases.len() >= 600, "{} cases", cases.len());
    let mut failed = Vec::new();
    for (case, expected) in &cases {
        let name = case[0].as_str().expect("a tool name");
        let found = outcome(name, &case[1]);
        if found != *expected {
            let shown = json::value_text(&found);
            failed.push(format!(
                "{name} {}\n got {shown}\nwant {}",
                case[1],
                json::value_text(expected)
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} cases differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

// Each choice of shapes tags its problems with a name the reference gives that shape.
#[test]
fn each_choice_tags_its_shapes_by_their_reference_names() {
    let choices: Vec<(&str, &str, Vec<String>)> = TOOLS
        .iter()
        .flat_map(|tool| tool.params.iter().map(|param| (tool.name, param)))
        .filter_map(|(tool, param)| match param.shape {
            Shape::AnyOf(shapes) => Some((tool, param.name, shapes.iter().map(tag).collect())),
            _ => None,
        })
        .collect();
    // Every choice parameter of the surface in tools/list order, and no other parameter.
    let want: Vec<(&str, &str, Vec<String>)> = [
        ("orient", "focus", ["str", "list[str]"]),
        ("read", "lines", ["list[int]", "list[list[int]]"]),
    ]
    .into_iter()
    .map(|(tool, name, tags)| (tool, name, tags.map(String::from).to_vec()))
    .collect();
    assert_eq!(choices, want);
}
