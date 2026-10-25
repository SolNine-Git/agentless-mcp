//! The operation checks of the tools with an operation table: the operation must exist, every
//! parameter it does not take must be absent, and every parameter it requires must be given.

use app::ops::limits::{MAX_MAX_FILES, MIN_MAX_FILES};
use app::surface::{Operation, Tool};
use prompts::messages::{
    ARGUMENT_INVALID, MAP_LIMIT_OUT_OF_RANGE, OPERATION_LACKS_PARAMETER, OPERATION_REQUIRES_VALUE,
};
use serde_json::Value as Json;

use crate::arguments::{Arguments, Value};
use crate::refusal::{accepted_by_name, operation_owner, shown};

// Every operation takes these, so no table row lists them.
const UNIVERSAL: [&str; 3] = ["repo_root", "operation", "no_cache"];

// The operation of `tool` named `name`, or the refusal that lists the operations `tool` has.
pub(crate) fn named_operation(tool: &Tool, name: &str) -> Result<&'static Operation, String> {
    tool.operation(name)
        .ok_or_else(|| unknown_operation(tool, name))
}

// The operations `tool` has, and the tool that has `name` when another one does.
fn unknown_operation(tool: &Tool, name: &str) -> String {
    let given = shown(&Json::from(name));
    let refusal =
        ARGUMENT_INVALID.render((&accepted_by_name(tool, "operation"), "operation", &given));
    match operation_owner(name, tool.name) {
        Some(owner) => format!("{refusal} {owner}"),
        None => refusal,
    }
}

// The sentence for a parameter that the call gives a value and `operation` does not take.
pub(crate) fn stray(operation: &Operation, param: &str, value: &Value) -> Option<String> {
    let taken = UNIVERSAL.contains(&param) || operation.accepted.contains(&param);
    (!taken && !omitted(value)).then(|| OPERATION_LACKS_PARAMETER.render((operation.name, param)))
}

// One sentence per parameter that `operation` requires and the call leaves absent.
pub(crate) fn missing(tool: &Tool, operation: &Operation, arguments: &Arguments) -> Vec<String> {
    let given: Vec<&str> = arguments
        .iter()
        .filter(|(param, value)| !UNIVERSAL.contains(param) && !omitted(value))
        .map(|(param, _)| *param)
        .collect();
    operation
        .required
        .iter()
        .filter(|param| !given.contains(param))
        .map(|param| {
            let form = accepted_by_name(tool, param);
            OPERATION_REQUIRES_VALUE.render((operation.name, &form, param))
        })
        .collect()
}

// A null, false, blank, zero or empty value says nothing, so it counts as absent.
pub(crate) fn omitted(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Text(text) => text::strip(text).is_empty(),
        Value::Int(number) => number.value() == Some(0),
        Value::Float(number) => number.partial_cmp(&0.0) == Some(std::cmp::Ordering::Equal),
        Value::List(items) => items.is_empty(),
    }
}

// The map's limit is its file cap, a narrower range than the listings that share the parameter.
pub(crate) fn checked_map_limit(
    operation: &Operation,
    arguments: &Arguments,
) -> Result<(), String> {
    let limit = arguments.int("limit").and_then(app::IntArg::value);
    match limit {
        Some(limit)
            if operation.name == "map" && !(MIN_MAX_FILES..=MAX_MAX_FILES).contains(&limit) =>
        {
            Err(MAP_LIMIT_OUT_OF_RANGE.render((limit, MIN_MAX_FILES, MAX_MAX_FILES)))
        }
        _ => Ok(()),
    }
}
