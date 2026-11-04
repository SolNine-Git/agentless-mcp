//! One tool call from its name and arguments to its answer: the tool, the validation with its
//! operation checks, the deadline, then the operation, with the three forms of a failed call.

use std::path::PathBuf;
use std::sync::Arc;

use app::Services;
use app::surface::{Operation, TOOLS, Tool};
use kernel::Deadline;
use serde_json::{Map, Value as Json};

use crate::answer::answer;
use crate::arguments::Arguments;
use crate::failure::Failure;
use crate::operation::checked_map_limit;
use crate::refusal::unknown_tool;
use crate::roots::Allowlist;
use crate::server::Ports;
use crate::validate::validate;

// The text of every failure the server did not plan for. The detail goes to the log only.
pub(crate) const UNPLANNED: &str = "the tool failed for a reason it does not handle; the server log \
     has the detail. This is a defect in agentless-mcp, not something the call can be corrected \
     to avoid.";

// What every call of one server reads.
pub(crate) struct Inner {
    pub(crate) allowlist: Allowlist,
    pub(crate) launch: Option<PathBuf>,
    pub(crate) call_limit: f64,
    pub(crate) recache_grace: f64,
    pub(crate) services: Arc<Services>,
    pub(crate) ports: Ports,
}

// A call that passed its checks: the tool, its operation, and the validated arguments.
pub(crate) struct Call {
    pub(crate) tool: &'static Tool,
    pub(crate) operation: Option<&'static Operation>,
    pub(crate) arguments: Arguments,
}

impl Call {
    pub(crate) fn operation_name(&self) -> &'static str {
        self.operation.map_or("", |operation| operation.name)
    }
}

// The text of an answer and whether it reports an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) text: String,
    pub(crate) error: bool,
}

// The call that `name` and `arguments` make, or the answer that refuses it before any repository
// opens.
pub(crate) fn prepared(
    inner: &Inner,
    name: &str,
    arguments: Option<Map<String, Json>>,
) -> Result<Call, Answer> {
    let Some(tool) = TOOLS.iter().copied().find(|tool| tool.name == name) else {
        return Err(failed(inner, name, Failure::Refused(unknown_tool(name))));
    };
    // Null arguments read as no arguments.
    let input = arguments.map_or_else(|| Json::Object(Map::new()), Json::Object);
    let (operation, arguments) =
        validate(tool, &input).map_err(|message| failed(inner, name, Failure::Refused(message)))?;
    if let Some(operation) = operation {
        checked_map_limit(operation, &arguments)
            .map_err(|message| failed(inner, name, Failure::Refused(message)))?;
    }
    Ok(Call {
        tool,
        operation,
        arguments,
    })
}

// The deadline a call starts with; capabilities works without one.
pub(crate) fn deadline(inner: &Inner, call: &Call) -> Option<Deadline> {
    (call.tool.name != "capabilities").then(|| {
        Deadline::new(
            inner.call_limit,
            inner.recache_grace,
            Arc::clone(&inner.services.clock),
        )
    })
}

// The answer of a call that passed its checks.
pub(crate) fn answered(inner: &Inner, call: &Call, deadline: Option<&Deadline>) -> Answer {
    match answer(inner, call, deadline) {
        Ok(text) => Answer { text, error: false },
        Err(failure) => failed(inner, call.tool.name, failure),
    }
}

// A refusal follows the tool's prefix; an unplanned failure shows one sentence and logs its detail.
pub(crate) fn failed(inner: &Inner, tool: &str, failure: Failure) -> Answer {
    let text = match failure {
        Failure::Refused(message) => format!("Error calling tool '{tool}': {message}"),
        Failure::Unplanned(detail) => {
            let line = format!("unhandled error in an agentless-mcp tool call: {tool}: {detail}");
            inner.ports.log.line(&line);
            UNPLANNED.to_owned()
        }
    };
    Answer { text, error: true }
}
