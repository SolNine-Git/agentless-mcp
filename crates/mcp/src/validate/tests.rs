use app::surface::{Presence, TOOLS, Tool};
use prompts::messages::ACCEPTS_SCHEMA;
use serde_json::{Map, Value as Json, json};

use super::validate;
use crate::arguments::Value;
use crate::refusal::accepted;

// The reference's coercion of one value of each shape that the tools take, from its outcomes for
// the removed tools' parameters of the same shape: [tool, parameter, value, outcome].
const LAX: &str = r#"[
["search", "limit", null, {"ok": null}],
["search", "limit", true, {"ok": 1}],
["search", "limit", false, {"error": "limit is false, but must be an integer from 1 to 1000."}],
["search", "limit", 1, {"ok": 1}],
["search", "limit", 2, {"ok": 2}],
["search", "limit", 7, {"ok": 7}],
["search", "limit", 500, {"ok": 500}],
["search", "limit", 2.0, {"ok": 2}],
["search", "limit", "3", {"ok": 3}],
["search", "limit", " 3 ", {"ok": 3}],
["search", "limit", "\t3\n", {"ok": 3}],
["search", "limit", "+3", {"ok": 3}],
["search", "limit", "3.0", {"ok": 3}],
["search", "limit", "3.00", {"ok": 3}],
["search", "limit", "3.", {"error": "limit is \"3.\", but must be an integer from 1 to 1000."}],
["search", "limit", ".5", {"error": "limit is \".5\", but must be an integer from 1 to 1000."}],
["search", "limit", "3.5", {"error": "limit is \"3.5\", but must be an integer from 1 to 1000."}],
["search", "limit", "1_0", {"ok": 10}],
["search", "limit", "1__0", {"error": "limit is \"1__0\", but must be an integer from 1 to 1000."}],
["search", "limit", "_10", {"error": "limit is \"_10\", but must be an integer from 1 to 1000."}],
["search", "limit", "10_", {"error": "limit is \"10_\", but must be an integer from 1 to 1000."}],
["search", "limit", "0x10", {"error": "limit is \"0x10\", but must be an integer from 1 to 1000."}],
["search", "limit", "0o7", {"error": "limit is \"0o7\", but must be an integer from 1 to 1000."}],
["search", "limit", "1e3", {"error": "limit is \"1e3\", but must be an integer from 1 to 1000."}],
["search", "limit", "", {"error": "limit is \"\", but must be an integer from 1 to 1000."}],
["search", "limit", " ", {"error": "limit is \" \", but must be an integer from 1 to 1000."}],
["search", "limit", "inf", {"error": "limit is \"inf\", but must be an integer from 1 to 1000."}],
["search", "limit", "nan", {"error": "limit is \"nan\", but must be an integer from 1 to 1000."}],
["search", "limit", "00012", {"ok": 12}],
["search", "limit", "0.0", {"error": "limit is \"0.0\", but must be an integer from 1 to 1000."}],
["search", "limit", "-0.0", {"error": "limit is \"-0.0\", but must be an integer from 1 to 1000."}],
["search", "limit", "3 4", {"error": "limit is \"3 4\", but must be an integer from 1 to 1000."}],
["search", "limit", "\u00a03", {"ok": 3}],
["search", "limit", "99999999999999999999999999999999999999999999999999999999999999999999999999999999", {"error": "limit is \"999999999999999999999999...99999999999999999999999\", but must be an integer from 1 to 1000."}],
["search", "limit", "1000000000000000000000000000000.0", {"error": "limit is \"1000000000000000000000000000000.0\", but must be an integer from 1 to 1000."}],
["search", "limit", "-_1", {"error": "limit is \"-_1\", but must be an integer from 1 to 1000."}],
["search", "limit", "+_1", {"error": "limit is \"+_1\", but must be an integer from 1 to 1000."}],
["search", "limit", "1_.0", {"error": "limit is \"1_.0\", but must be an integer from 1 to 1000."}],
["search", "limit", "1._0", {"error": "limit is \"1._0\", but must be an integer from 1 to 1000."}],
["search", "limit", "1.0_0", {"error": "limit is \"1.0_0\", but must be an integer from 1 to 1000."}],
["search", "limit", "1_0.0", {"ok": 10}],
["search", "limit", "\u001c3", {"error": "limit is \"\\u001c3\", but must be an integer from 1 to 1000."}],
["search", "limit", "3\u001f", {"error": "limit is \"3\\u001f\", but must be an integer from 1 to 1000."}],
["search", "limit", "\u30003", {"ok": 3}],
["search", "limit", [], {"error": "limit is [], but must be an integer from 1 to 1000."}],
["search", "limit", [1], {"error": "limit is [1], but must be an integer from 1 to 1000."}],
["search", "limit", {}, {"error": "limit is {}, but must be an integer from 1 to 1000."}],
["search", "limit", {"a": 1}, {"error": "limit is {\"a\":1}, but must be an integer from 1 to 1000."}],
["search", "ignore_case", null, {"ok": null}],
["search", "ignore_case", true, {"ok": true}],
["search", "ignore_case", false, {"ok": false}],
["search", "ignore_case", 0, {"ok": false}],
["search", "ignore_case", 1, {"ok": true}],
["search", "ignore_case", 2, {"error": "ignore_case is 2, but must be true or false."}],
["search", "ignore_case", -1, {"error": "ignore_case is -1, but must be true or false."}],
["search", "ignore_case", 0.0, {"ok": false}],
["search", "ignore_case", 1.0, {"ok": true}],
["search", "ignore_case", 1.5, {"error": "ignore_case is 1.5, but must be true or false."}],
["search", "ignore_case", "true", {"ok": true}],
["search", "ignore_case", "false", {"ok": false}],
["search", "ignore_case", "True", {"ok": true}],
["search", "ignore_case", "FALSE", {"ok": false}],
["search", "ignore_case", "yes", {"ok": true}],
["search", "ignore_case", "no", {"ok": false}],
["search", "ignore_case", "on", {"ok": true}],
["search", "ignore_case", "off", {"ok": false}],
["search", "ignore_case", "1", {"ok": true}],
["search", "ignore_case", "0", {"ok": false}],
["search", "ignore_case", "t", {"ok": true}],
["search", "ignore_case", "f", {"ok": false}],
["search", "ignore_case", "y", {"ok": true}],
["search", "ignore_case", "n", {"ok": false}],
["search", "ignore_case", "Yes", {"ok": true}],
["search", "ignore_case", "YES", {"ok": true}],
["search", "ignore_case", " true", {"error": "ignore_case is \" true\", but must be true or false."}],
["search", "ignore_case", "true ", {"error": "ignore_case is \"true \", but must be true or false."}],
["search", "ignore_case", "maybe", {"error": "ignore_case is \"maybe\", but must be true or false."}],
["search", "ignore_case", "", {"error": "ignore_case is \"\", but must be true or false."}],
["search", "ignore_case", "2", {"error": "ignore_case is \"2\", but must be true or false."}],
["search", "ignore_case", "0.0", {"error": "ignore_case is \"0.0\", but must be true or false."}],
["search", "ignore_case", [], {"error": "ignore_case is [], but must be true or false."}],
["search", "ignore_case", {}, {"error": "ignore_case is {}, but must be true or false."}],
["search", "ignore_case", [true], {"error": "ignore_case is [true], but must be true or false."}],
["search", "path", null, {"ok": null}],
["search", "path", true, {"error": "path is true, but must be a string."}],
["search", "path", 0, {"error": "path is 0, but must be a string."}],
["search", "path", 1.5, {"error": "path is 1.5, but must be a string."}],
["search", "path", "", {"ok": ""}],
["search", "path", " ", {"ok": " "}],
["search", "path", "map", {"ok": "map"}],
["search", "path", "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", {"ok": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}],
["search", "path", "a'b", {"ok": "a'b"}],
["search", "path", "a\"b", {"ok": "a\"b"}],
["search", "path", "a'b\"c", {"ok": "a'b\"c"}],
["search", "path", "line\nbreak", {"ok": "line\nbreak"}],
["search", "path", "tab\t", {"ok": "tab\t"}],
["search", "path", "\u0000", {"ok": "\u0000"}],
["search", "path", "\u001f\u007f", {"ok": "\u001f\u007f"}],
["search", "path", "\u0080\u009f\u00a0", {"ok": "\u0080\u009f\u00a0"}],
["search", "path", "\u00e9", {"ok": "\u00e9"}],
["search", "path", "\u65e5\u672c", {"ok": "\u65e5\u672c"}],
["search", "path", "\u200b", {"ok": "\u200b"}],
["search", "path", "\u2028", {"ok": "\u2028"}],
["search", "path", "\ud83d\ude00", {"ok": "\ud83d\ude00"}],
["search", "path", "\\", {"ok": "\\"}],
["search", "path", [], {"error": "path is [], but must be a string."}],
["search", "path", ["map"], {"error": "path is [\"map\"], but must be a string."}],
["search", "path", {}, {"error": "path is {}, but must be a string."}],
["search", "path", {"a": [1, 2]}, {"error": "path is {\"a\":[1,2]}, but must be a string."}],
["trace", "target", "", {"ok": ""}],
["trace", "target", "a", {"ok": "a"}],
["trace", "target", ["a"], {"ok": ["a"]}],
["trace", "target", ["a", "b"], {"ok": ["a", "b"]}],
["trace", "target", ["a", 5], {"error": "target is [\"a\",5], but must be a string or a list of strings."}],
["trace", "target", [5, 6], {"error": "target is [5,6], but must be a string or a list of strings."}],
["trace", "target", 5, {"error": "target is 5, but must be a string or a list of strings."}],
["trace", "target", true, {"error": "target is true, but must be a string or a list of strings."}],
["trace", "target", {}, {"error": "target is {}, but must be a string or a list of strings."}],
["trace", "target", 1.5, {"error": "target is 1.5, but must be a string or a list of strings."}],
["trace", "target", [null], {"error": "target is [null], but must be a string or a list of strings."}],
["trace", "target", [[]], {"error": "target is [[]], but must be a string or a list of strings."}]
]"#;

fn tool_named(name: &str) -> &'static Tool {
    TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
}

fn value_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(flag) => Json::Bool(*flag),
        Value::Int(number) => serde_json::from_str(&number.to_string()).expect("an integer"),
        Value::Float(number) => json!(number),
        Value::Text(text) => Json::String(text.clone()),
        Value::List(items) => Json::Array(items.iter().map(value_json).collect()),
    }
}

// The coerced value of `param` alone, or the refusal; a trace call names a target too.
fn outcome(tool: &str, param: &str, value: &Json) -> Json {
    let mut call = Map::new();
    if tool == "trace" && param != "target" {
        call.insert("target".to_owned(), json!("x"));
    }
    call.insert(param.to_owned(), value.clone());
    match validate(tool_named(tool), &Json::Object(call)) {
        Ok(arguments) => json!({"ok": value_json(arguments.value(param))}),
        Err(refusal) => json!({"error": refusal}),
    }
}

#[test]
fn each_value_coerces_as_the_reference_coerces_its_shape() {
    let cases: Vec<(String, String, Json, Json)> = serde_json::from_str(LAX).expect("the cases");
    assert_eq!(cases.len(), 121);
    let failed: Vec<String> = (cases.iter())
        .filter_map(|(tool, param, value, want)| {
            let found = outcome(tool, param, value);
            (found != *want).then(|| format!("{tool}.{param} {value}\n got {found}\nwant {want}"))
        })
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

// An integer text of up to 4300 digits converts, a longer one does not; both are refused here.
#[test]
fn a_long_integer_text_is_refused_with_its_head_and_tail() {
    let refusal = |head: &str| {
        format!(
            "limit is \"{head}{}...{}\", but must be an integer from 1 to 1000.",
            "9".repeat(24 - head.len()),
            "9".repeat(23)
        )
    };
    for (text, head) in [
        ("9".repeat(4300), "99"),
        ("9".repeat(4301), "99"),
        ("9".repeat(5000), "99"),
        (format!("+{}", "9".repeat(4299)), "+9"),
    ] {
        let found = outcome("search", "limit", &json!(text));
        assert_eq!(found, json!({"error": refusal(head)}), "{}", text.len());
    }
}

// Every parameter of the surface states its accepted form in words, and no refusal falls back to
// pointing at the schema.
#[test]
fn each_parameter_states_its_accepted_form_in_words() {
    for tool in TOOLS {
        for param in tool.params {
            let form = accepted(param);
            assert_ne!(form, ACCEPTS_SCHEMA, "{}.{}", tool.name, param.name);
        }
    }
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
        let absent = validate(tool, &Json::Object(call.clone())).expect("a valid call");
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
            let found = validate(tool, &Json::Object(given)).expect("a valid call");
            assert_eq!(found, absent, "{}.{name}", tool.name);
        }
    }
}

fn assert_refusals(cases: &[(&str, Json, &str)]) {
    for (name, arguments, want) in cases {
        let found = validate(tool_named(name), arguments);
        assert_eq!(found.err().as_deref(), Some(*want), "{name} {arguments}");
    }
}

// Each parameter in declaration order gets at most one sentence, the unknown names follow in the
// call's order, and the listing of what the tool takes comes last.
#[test]
fn a_refusal_judges_each_parameter_once_in_declaration_order() {
    assert_refusals(&[
        (
            "trace",
            json!({}),
            "target is required and must be a string or a list of strings.",
        ),
        (
            "trace",
            json!({"more": "bodies", "target": 3, "operation": "find"}),
            "target is 3, but must be a string or a list of strings. more is \"bodies\", but \
             must be one of candidates, callers, callees, tests, similar, history, body, \
             declares, imports, importers, cycles or tree. trace has no parameter operation. \
             trace takes target, repo_root and more.",
        ),
        (
            "trace",
            json!({"target": ["a", "b", "c", "d", "e", "f", "g", "h", "i"]}),
            "target is [\"a\",\"b\",\"c\",\"d\",\"e\",\"f\",\"g\",\"h\",\"i\"], but must be a \
             string or a list of strings.",
        ),
        (
            "search",
            json!({"context": 11, "limit": 0, "glob": "*.rs"}),
            "context is 11, but must be an integer from 0 to 10. limit is 0, but must be an \
             integer from 1 to 1000. search has no parameter glob. search takes pattern, \
             repo_root, files, path, ignore_case, context and limit.",
        ),
        // The MCP tools have no no_cache parameter: the CLI keeps --no-cache.
        (
            "search",
            json!({"pattern": "x", "no_cache": true}),
            "search has no parameter no_cache. search takes pattern, repo_root, files, path, \
             ignore_case, context and limit.",
        ),
    ]);
}
