use std::fs;
use std::path::Path;

use app::surface::{Presence, TOOLS, Tool};
use json::JsonValue;
use prompts::messages::{ACCEPTS_LINE_RANGES, ACCEPTS_SCHEMA};
use serde_json::{Map, Value as Json, json};
use text::CodePointString;

use super::{unknown_names, validate, verdicts};
use crate::arguments::Value;
use crate::refusal::accepted;

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

fn tool_named(name: &str) -> &'static Tool {
    TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
}

// The coerced values when each value fits its shape and the tool takes each name, else the
// refusal. Many accepted rows name an operation that refuses the call, so they test the coercion.
fn outcome(name: &str, arguments: &Json) -> JsonValue {
    let tool = tool_named(name);
    let empty = Map::new();
    let given = arguments.as_object().unwrap_or(&empty);
    let fitting: Option<Vec<(&str, Value)>> = verdicts(tool, given)
        .into_iter()
        .map(|(name, verdict)| verdict.ok().map(|value| (name, value)))
        .collect();
    match fitting {
        Some(values) if unknown_names(tool, given).is_empty() => {
            let entries = values
                .iter()
                .map(|(name, value)| (key(name), value_json(value)));
            JsonValue::Object(vec![(key("ok"), JsonValue::Object(entries.collect()))])
        }
        _ => {
            let text = validate(tool, arguments).expect_err("a value that does not fit");
            JsonValue::Object(vec![(key("error"), JsonValue::Str(key(&text)))])
        }
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

// Every parameter of the surface states its accepted form in words, and no refusal falls back to
// pointing at the schema.
#[test]
fn each_parameter_states_its_accepted_form_in_words() {
    for tool in TOOLS {
        for param in tool.params {
            let form = accepted(tool, param);
            assert_ne!(form, ACCEPTS_SCHEMA, "{}.{}", tool.name, param.name);
        }
    }
    let read = TOOLS.iter().find(|tool| tool.name == "read").unwrap();
    let lines = read
        .params
        .iter()
        .find(|param| param.name == "lines")
        .unwrap();
    assert_eq!(accepted(read, lines), ACCEPTS_LINE_RANGES);
}

// tools/list offers no null choice, and an explicit null still reads as an absent value.
#[test]
fn an_explicit_null_reads_as_absent_for_each_nullable_parameter() {
    for tool in TOOLS {
        let required = tool
            .params
            .iter()
            .filter(|param| param.presence == Presence::Required);
        let call: Map<String, Json> = required
            .map(|param| (param.name.to_owned(), Json::from("x")))
            .collect();
        let absent = verdicts(tool, &call);
        let fits = absent.iter().all(|(_, verdict)| verdict.is_ok());
        assert!(fits, "{}: {absent:?}", tool.name);
        let nullable: Vec<&str> = tool
            .params
            .iter()
            .filter(|param| param.presence == Presence::Nullable)
            .map(|param| param.name)
            .collect();
        assert!(!nullable.is_empty(), "{}", tool.name);
        for name in nullable {
            let mut given = call.clone();
            given.insert(name.to_owned(), Json::Null);
            assert_eq!(verdicts(tool, &given), absent, "{}.{name}", tool.name);
        }
    }
}

fn assert_refusals(cases: &[(&str, Json, &str)]) {
    for (name, arguments, want) in cases {
        let found = validate(tool_named(name), arguments);
        assert_eq!(found.err().as_deref(), Some(*want), "{name} {arguments}");
    }
}

// A bad operation is the whole refusal: the other parameters have no operation to be judged by.
#[test]
fn a_refusal_decides_the_operation_first() {
    assert_refusals(&[
        (
            "orient",
            json!({"operation": "find", "name": "x"}),
            "operation is \"find\", but must be map. Call the tool symbols with operation find.",
        ),
        (
            "symbols",
            json!({"operation": "map", "focus": "x"}),
            "operation is \"map\", but must be one of find, overview, expand, explain or locate. \
             Call the tool orient with operation map.",
        ),
        (
            "orient",
            json!({"operation": "bogus", "name": "x"}),
            "operation is \"bogus\", but must be map.",
        ),
        (
            "orient",
            json!({"operation": "communities"}),
            "operation is \"communities\", but must be map. Call the tool graph with operation \
             communities.",
        ),
        (
            "orient",
            json!({"operation": "path", "source": "a", "target": "b"}),
            "operation is \"path\", but must be map. Call the tool graph with operation path.",
        ),
        (
            "graph",
            json!({"operation": "map"}),
            "operation is \"map\", but must be one of communities, cycles, health, diagram or \
             path. Call the tool orient with operation map.",
        ),
        (
            "orient",
            json!({}),
            "operation is required and must be map.",
        ),
        (
            "symbols",
            json!({"name": "x"}),
            "operation is required and must be one of find, overview, expand, explain or locate.",
        ),
        (
            "symbols",
            json!({"operation": null, "name": "x"}),
            "operation is null, but must be one of find, overview, expand, explain or locate.",
        ),
    ]);
}

// Each parameter in declaration order gets at most one sentence, the unknown names follow in the
// call's order, and the listing of what the operation takes comes last.
#[test]
fn a_refusal_judges_each_parameter_once_in_declaration_order() {
    assert_refusals(&[
        (
            "symbols",
            json!({"operation": "find", "name": "x", "path": "a", "lines": [1, 2]}),
            "Operation find has no parameter path. symbols has no parameter lines. Operation find \
             takes name, kind and limit, and requires name.",
        ),
        (
            "symbols",
            json!({"operation": "find", "name": 3, "path": "a"}),
            "name is 3, but must be a string. Operation find has no parameter path. Operation \
             find takes name, kind and limit, and requires name.",
        ),
        (
            "symbols",
            json!({"operation": "find", "name": "x", "path": "a"}),
            "Operation find has no parameter path. Operation find takes name, kind and limit, \
             and requires name.",
        ),
        (
            "read",
            json!({"operation": "dir", "deep": 2}),
            "read has no parameter deep. Operation dir takes path, depth and max_entries.",
        ),
        // A stray value that does not fit its shape gets the shape sentence, not the stray one.
        (
            "symbols",
            json!({"operation": "find", "name": "x", "path": 3}),
            "path is 3, but must be a string.",
        ),
        (
            "graph",
            json!({"operation": "cycles", "limit": 0, "focus": "x"}),
            "Operation cycles has no parameter focus. limit is 0, but must be an integer from 1 \
             to 500. Operation cycles takes limit.",
        ),
        // The MCP tools have no no_cache parameter: the CLI keeps --no-cache.
        (
            "orient",
            json!({"operation": "map", "no_cache": true}),
            "orient has no parameter no_cache. Operation map takes focus, budget, limit and \
             granularity.",
        ),
    ]);
}

// A false, zero or empty value says nothing, so it is no stray for an operation that does not
// take its parameter. A name that the tool lacks is refused whatever its value.
#[test]
fn an_empty_value_is_no_stray_but_an_unknown_name_is_refused() {
    let call = json!({"operation": "cycles", "include_unique": false, "max_edges": 0, "focus": []});
    assert!(validate(tool_named("graph"), &call).is_ok(), "{call}");
    assert_refusals(&[(
        "orient",
        json!({"operation": "map", "include_unique": false, "max_edges": 0, "focus": []}),
        "orient has no parameter include_unique. orient has no parameter max_edges. Operation \
         map takes focus, budget, limit and granularity.",
    )]);
}
