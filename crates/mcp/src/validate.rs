//! The argument validation of one tool call: each value coerced to its parameter's shape, in
//! declaration order, the operation checks, and the refusal text of a call whose values do not fit.

mod coerce;
#[cfg(test)]
mod tests;

use app::surface::{Calls, Operation, Param, Presence, Tool};
use prompts::messages::{
    ARGUMENT_INVALID, ARGUMENT_MISSING, ARGUMENT_UNKNOWN, LISTED_AND, TOOL_TAKES,
};
use serde_json::{Map, Value as Json};

use self::coerce::coerce;
use crate::arguments::{Arguments, Value};
use crate::operation::{missing, named_operation, stray};
use crate::refusal::{accepted, listed, operation_takes, shown};

// Each parameter's value coerced to its shape, or the sentence that refuses the value.
type Verdicts = Vec<(&'static str, Result<Value, String>)>;

// The operation and arguments of a call, or the refusal: a bad operation alone, else one sentence
// per bad parameter and unknown name, and the missing parameters only when nothing else is wrong.
pub(crate) fn validate(
    tool: &'static Tool,
    input: &Json,
) -> Result<(Option<&'static Operation>, Arguments), String> {
    let empty = Map::new();
    let given = match input {
        Json::Object(entries) => entries,
        _ => &empty,
    };
    let operation = decided_operation(tool, given)?;
    let (values, mut sentences, strayed) = judged(operation, verdicts(tool, given));
    let unknown = unknown_names(tool, given);
    let listing = strayed || !unknown.is_empty();
    sentences.extend(unknown);
    if listing {
        sentences.push(takes(tool, operation));
    }
    let arguments = Arguments::new(values);
    if let (true, Some(operation)) = (sentences.is_empty(), operation) {
        sentences = missing(tool, operation, &arguments);
    }
    if sentences.is_empty() {
        Ok((operation, arguments))
    } else {
        Err(sentences.join(" "))
    }
}

// `None` for a tool with no operation table, else the operation the call names or the refusal of
// a missing, malformed or unknown operation.
fn decided_operation(
    tool: &'static Tool,
    given: &Map<String, Json>,
) -> Result<Option<&'static Operation>, String> {
    let Calls::Operations(_) = tool.calls else {
        return Ok(None);
    };
    let param = tool.params.iter().find(|param| param.name == "operation");
    let param = param.expect("each tool with an operation table declares operation");
    let Value::Text(name) = param_value(tool, param, given.get(param.name))? else {
        unreachable!("operation is a required text parameter");
    };
    named_operation(tool, &name).map(Some)
}

fn verdicts(tool: &Tool, given: &Map<String, Json>) -> Verdicts {
    tool.params
        .iter()
        .map(|param| (param.name, param_value(tool, param, given.get(param.name))))
        .collect()
}

// The values that fit, one sentence per value that does not fit or that the operation does not
// take, and whether any sentence is of the second kind.
fn judged(
    operation: Option<&Operation>,
    verdicts: Verdicts,
) -> (Vec<(&'static str, Value)>, Vec<String>, bool) {
    let mut values = Vec::new();
    let mut sentences = Vec::new();
    let mut strayed = false;
    for (name, verdict) in verdicts {
        let sentence = match verdict {
            Ok(value) => {
                let found = operation.and_then(|operation| stray(operation, name, &value));
                strayed |= found.is_some();
                values.push((name, value));
                found
            }
            Err(sentence) => Some(sentence),
        };
        sentences.extend(sentence);
    }
    (values, sentences, strayed)
}

// A required parameter must be given; a nullable one reads null when absent or null.
fn param_value(tool: &Tool, param: &Param, given: Option<&Json>) -> Result<Value, String> {
    match (given, param.presence) {
        (None, Presence::Required) => {
            Err(ARGUMENT_MISSING.render((&accepted(tool, param), param.name)))
        }
        (None, Presence::DefaultFalse) => Ok(Value::Bool(false)),
        (None | Some(Json::Null), Presence::Nullable) => Ok(Value::Null),
        (Some(value), _) => coerce(&param.shape, value).ok_or_else(|| {
            ARGUMENT_INVALID.render((&accepted(tool, param), param.name, &shown(value)))
        }),
    }
}

// One sentence per name that `tool` does not take, in the order the call gives them.
fn unknown_names(tool: &Tool, given: &Map<String, Json>) -> Vec<String> {
    given
        .keys()
        .filter(|name| !tool.params.iter().any(|param| param.name == name.as_str()))
        .map(|name| ARGUMENT_UNKNOWN.render((tool.name, name)))
        .collect()
}

// What the call's operation takes, or for a tool with no operation table, what the tool takes.
fn takes(tool: &Tool, operation: Option<&Operation>) -> String {
    operation.map_or_else(
        || {
            let names: Vec<&str> = tool.params.iter().map(|param| param.name).collect();
            TOOL_TAKES.render((tool.name, &listed(&names, &LISTED_AND)))
        },
        operation_takes,
    )
}
