//! The argument validation of one tool call: each value coerced to its parameter's shape, in
//! declaration order, and the refusal text of a call whose values do not fit.

mod coerce;
mod repr;
#[cfg(test)]
mod tests;

use app::surface::{Param, Presence, Tool};
use serde_json::{Map, Value as Json};

use self::coerce::{Coerced, Problem, coerce, problem};
use self::repr::{shown, type_name};
use crate::arguments::{Arguments, Value};

const DOCS: &str = "https://errors.pydantic.dev/2.13/v/";

// The arguments of `tool` from the JSON object `input`, or the refusal text that lists each
// problem: the parameters in declaration order, then each name the tool does not take.
pub(crate) fn validate(tool: &'static Tool, input: &Json) -> Result<Arguments, String> {
    let empty = Map::new();
    let given = match input {
        Json::Object(entries) => entries,
        _ => &empty,
    };
    let mut values = Vec::new();
    let mut problems = Vec::new();
    for param in tool.params {
        match param_value(param, given.get(param.name), input) {
            Ok(value) => values.push((param.name, value)),
            Err(mut failed) => problems.append(&mut failed),
        }
    }
    let unexpected = given
        .iter()
        .filter(|(name, _)| !tool.params.iter().any(|param| param.name == name.as_str()));
    for (name, value) in unexpected {
        let message = "Unexpected keyword argument";
        problems.append(&mut problem(
            name,
            "unexpected_keyword_argument",
            message,
            value,
        ));
    }
    if problems.is_empty() {
        Ok(Arguments::new(values))
    } else {
        Err(report(tool.name, &problems))
    }
}

// A required parameter must be given; a nullable one reads null when absent or null.
fn param_value<'a>(param: &Param, given: Option<&'a Json>, input: &'a Json) -> Coerced<'a> {
    match (given, param.presence) {
        (None, Presence::Required) => Err(problem(
            param.name,
            "missing_argument",
            "Missing required argument",
            input,
        )),
        (None, Presence::DefaultFalse) => Ok(Value::Bool(false)),
        (None | Some(Json::Null), Presence::Nullable) => Ok(Value::Null),
        (Some(value), _) => coerce(&param.shape, value, param.name),
    }
}

fn report(tool: &str, problems: &[Problem<'_>]) -> String {
    let count = problems.len();
    let noun = if count == 1 { "error" } else { "errors" };
    let mut lines = vec![format!("{count} validation {noun} for call[{tool}]")];
    for found in problems {
        lines.push(found.loc.clone());
        lines.push(format!(
            "  {} [type={}, input_value={}, input_type={}]",
            found.message,
            found.kind,
            shown(found.input),
            type_name(found.input)
        ));
        lines.push(format!(
            "    For further information visit {DOCS}{}",
            found.kind
        ));
    }
    lines.join("\n")
}
