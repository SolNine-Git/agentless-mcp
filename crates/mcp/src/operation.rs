//! The operation checks of the tools with an operation table: the operation must exist, every
//! parameter it does not take must be absent, and every parameter it requires must be given.

use app::ops::limits::{MAX_MAX_FILES, MIN_MAX_FILES};
use app::surface::{Calls, Operation, Tool};
use prompts::messages::{
    MAP_LIMIT_OUT_OF_RANGE, OP_REJECTS_PARAMETERS, OP_REQUIRES_PARAMETERS, UNKNOWN_OPERATION,
};

use crate::arguments::{Arguments, Value};

// Every operation takes these, so no table row lists them.
const UNIVERSAL: [&str; 3] = ["repo_root", "operation", "no_cache"];

// The operation the call names, `None` for a tool with no table, or the refusal text.
pub(crate) fn checked_operation(
    tool: &'static Tool,
    arguments: &Arguments,
) -> Result<Option<&'static Operation>, String> {
    let Calls::Operations(table) = tool.calls else {
        return Ok(None);
    };
    let name = arguments
        .text("operation")
        .expect("the validation gives each tool with an operation table a text operation");
    let Some(operation) = tool.operation(name) else {
        let mut names: Vec<&str> = table.iter().map(|operation| operation.name).collect();
        names.sort_unstable();
        return Err(UNKNOWN_OPERATION.render((tool.name, name, &names.join(", "))));
    };
    let given: Vec<&str> = arguments
        .iter()
        .filter(|(param, value)| !UNIVERSAL.contains(param) && !omitted(value))
        .map(|(param, _)| *param)
        .collect();
    refused_parameters(tool, operation, &given).map_or(Ok(Some(operation)), Err)
}

// The stray parameters refused first, then the missing ones.
fn refused_parameters(tool: &Tool, operation: &Operation, given: &[&str]) -> Option<String> {
    let accepted = operation.accepted.join(", ");
    let required = if operation.required.is_empty() {
        "none".to_owned()
    } else {
        operation.required.join(", ")
    };
    let mut stray: Vec<&str> = given
        .iter()
        .copied()
        .filter(|param| !operation.accepted.contains(param))
        .collect();
    stray.sort_unstable();
    if !stray.is_empty() {
        let values = (
            tool.name,
            operation.name,
            &*stray.join(", "),
            &*accepted,
            &*required,
        );
        return Some(OP_REJECTS_PARAMETERS.render(values));
    }
    let missing: Vec<&str> = operation
        .required
        .iter()
        .copied()
        .filter(|param| !given.contains(param))
        .collect();
    (!missing.is_empty()).then(|| {
        let values = (
            tool.name,
            operation.name,
            &*missing.join(", "),
            &*accepted,
            &*required,
        );
        OP_REQUIRES_PARAMETERS.render(values)
    })
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
