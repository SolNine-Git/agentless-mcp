//! The argument validation of one tool call: each value coerced to its parameter's shape, in
//! declaration order, and the refusal text of a call whose values do not fit.

mod coerce;
#[cfg(test)]
mod tests;

use app::surface::{Param, Presence, Tool};
use prompts::messages::{
    ARGUMENT_INVALID, ARGUMENT_MISSING, ARGUMENT_UNKNOWN, LISTED_AND, TOOL_TAKES,
};
use serde_json::{Map, Value as Json};

use self::coerce::coerce;
use crate::arguments::{Arguments, Value};
use crate::refusal::{accepted, listed, shown};

// The arguments of a call, or the refusal: one sentence per bad parameter and unknown name, and
// after an unknown name, what the tool takes.
pub(crate) fn validate(tool: &'static Tool, input: &Json) -> Result<Arguments, String> {
    let empty = Map::new();
    let given = match input {
        Json::Object(entries) => entries,
        _ => &empty,
    };
    let mut values = Vec::new();
    let mut sentences = Vec::new();
    for param in tool.params {
        match param_value(param, given.get(param.name)) {
            Ok(value) => values.push((param.name, value)),
            Err(sentence) => sentences.push(sentence),
        }
    }
    let unknown = unknown_names(tool, given);
    if !unknown.is_empty() {
        sentences.extend(unknown);
        sentences.push(takes(tool));
    }
    if sentences.is_empty() {
        Ok(Arguments::new(values))
    } else {
        Err(sentences.join(" "))
    }
}

// A required parameter must be given; a nullable one reads null when absent or null.
fn param_value(param: &Param, given: Option<&Json>) -> Result<Value, String> {
    match (given, param.presence) {
        (None, Presence::Required) => Err(ARGUMENT_MISSING.render((&accepted(param), param.name))),
        (None, Presence::DefaultFalse) => Ok(Value::Bool(false)),
        (None | Some(Json::Null), Presence::Nullable) => Ok(Value::Null),
        (Some(value), _) => coerce(&param.shape, value)
            .ok_or_else(|| ARGUMENT_INVALID.render((&accepted(param), param.name, &shown(value)))),
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

fn takes(tool: &Tool) -> String {
    let names: Vec<&str> = tool.params.iter().map(|param| param.name).collect();
    TOOL_TAKES.render((tool.name, &listed(&names, &LISTED_AND)))
}
