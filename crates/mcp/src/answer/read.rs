//! The read operations: a slice of one file's lines, and the directory tree.

use app::ops::limits::{DEFAULT_CONTEXT_LINES, DEFAULT_TREE_DEPTH, DEFAULT_TREE_ENTRIES};
use app::{IntArg, SliceRequest, TreeRequest};
use prompts::messages::{
    ACCEPTS_LINE_RANGES, SLICE_LINES_AND_WHOLE_FILE, SLICE_NEEDS_LINES, SLICE_RANGE_REVERSED,
};
use render::{ToolForm, tool_text};

use super::Context;
use crate::arguments::{Arguments, Value};
use crate::dispatch::Call;
use crate::failure::Failure;

// The operation that the log detail of a range with no pair names.
const SLICE: &str = "read operation 'slice'";

pub(super) fn answer(context: &Context<'_>, call: &Call) -> Result<String, Failure> {
    let arguments = &call.arguments;
    match call.operation_name() {
        "slice" => slice(context, arguments),
        "dir" => {
            let request = TreeRequest {
                depth: arguments.int_or("depth", DEFAULT_TREE_DEPTH),
                max_entries: arguments.int_or("max_entries", DEFAULT_TREE_ENTRIES),
            };
            let services = &*context.inner.services;
            let repo = context.reading.repo;
            context.viewed(app::subtree(
                services,
                repo,
                arguments.text("path"),
                &request,
            ))
        }
        other => Err(Failure::Unplanned(format!("read has no step for {other}"))),
    }
}

// A slice that gives no line fails with the reason: the path does not read, or every range starts
// past the end of the file.
fn slice(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = SliceRequest {
        path: arguments.text_or_empty("path"),
        ranges: slice_ranges(arguments)?,
        context: arguments.int_or("context_lines", DEFAULT_CONTEXT_LINES),
    };
    let outcome = app::slice(&context.reading, &request);
    context.logged(&outcome.diagnostics);
    let view = outcome.value?;
    if !view.error.is_empty() {
        return Err(Failure::Refused(view.error));
    }
    Ok(context.rendered(|receipt| tool_text(receipt, ToolForm::Slice(&view))))
}

// A slice is bounded by its line ranges or asks for the whole file, never both or neither.
fn slice_ranges(arguments: &Arguments) -> Result<Vec<(usize, usize)>, Failure> {
    let whole = arguments.flag("whole_file");
    let lines = arguments.value("lines");
    match lines {
        Value::List(_) if whole => Err(Failure::Refused(SLICE_LINES_AND_WHOLE_FILE.to_owned())),
        Value::List(items) => line_ranges(lines, items),
        _ if whole => Ok(Vec::new()),
        _ => Err(Failure::Refused(
            SLICE_NEEDS_LINES.render(ACCEPTS_LINE_RANGES),
        )),
    }
}

// The validation gives one pair of line numbers or a list of pairs, and one pair is one range.
fn line_ranges(lines: &Value, items: &[Value]) -> Result<Vec<(usize, usize)>, Failure> {
    match items {
        [Value::Int(_), Value::Int(_)] => Ok(vec![line_range(lines)?]),
        pairs => pairs.iter().map(line_range).collect(),
    }
}

// The validation gives each range two line numbers of at least 1; the end must not come first.
fn line_range(pair: &Value) -> Result<(usize, usize), Failure> {
    let ends: Vec<&IntArg> = match pair {
        Value::List(ends) => ends
            .iter()
            .filter_map(|end| match end {
                Value::Int(number) => Some(number),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let [start, end] = ends[..] else {
        return Err(Failure::Unplanned(format!(
            "{SLICE} range {pair:?} is no pair"
        )));
    };
    if !ordered(start, end) {
        let given = format!("[{start},{end}]");
        return Err(Failure::Refused(SLICE_RANGE_REVERSED.render(&given)));
    }
    Ok((line(start), line(end)))
}

// Whether `end` is at or after `start` by their exact values, which may lie past usize. Both are
// at least 1, so the longer decimal text is the larger number.
fn ordered(start: &IntArg, end: &IntArg) -> bool {
    let (start, end) = (start.to_string(), end.to_string());
    (start.len(), &start) <= (end.len(), &end)
}

fn line(number: &IntArg) -> usize {
    number.value().unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests;
