//! A settings document as Claude Code lays one out: one item per line, two spaces of indent per
//! level, `": "` after each key, and an empty container on one line. The keys keep their order,
//! and each scalar is the JSON text that the json crate writes for it.

use json::JsonValue;
use text::CodePointString;

const INDENT: &str = "  ";

// One piece of output still to write. A container pushes its pieces last first.
enum Step<'a> {
    Value(&'a JsonValue, usize),
    Key(&'a CodePointString),
    Text(&'static str),
    Line(usize),
}

// A work stack, not recursion: a parsed value can nest 9998 levels deep.
pub(crate) fn indented(value: &JsonValue) -> String {
    let mut text = String::new();
    let mut pending = vec![Step::Value(value, 0)];
    while let Some(step) = pending.pop() {
        match step {
            Step::Value(value, depth) => push_value(&mut text, value, depth, &mut pending),
            Step::Key(key) => text.push_str(&json::value_text(&JsonValue::Str(key.clone()))),
            Step::Text(raw) => text.push_str(raw),
            Step::Line(depth) => {
                text.push('\n');
                text.push_str(&INDENT.repeat(depth));
            }
        }
    }
    text.push('\n');
    text
}

fn push_value<'a>(
    text: &mut String,
    value: &'a JsonValue,
    depth: usize,
    pending: &mut Vec<Step<'a>>,
) {
    match value {
        JsonValue::Array(items) if !items.is_empty() => {
            text.push('[');
            let items = items.iter().map(|item| vec![Step::Value(item, depth + 1)]);
            push_items(pending, "]", depth, items);
        }
        JsonValue::Object(pairs) if !pairs.is_empty() => {
            text.push('{');
            let pairs = pairs.iter().map(|(key, item)| {
                vec![
                    Step::Key(key),
                    Step::Text(": "),
                    Step::Value(item, depth + 1),
                ]
            });
            push_items(pending, "}", depth, pairs);
        }
        scalar => text.push_str(&json::value_text(scalar)),
    }
}

// Pushes the closer on its own line, then each item on a line of its own, last first, with a
// comma after each item but the last.
fn push_items<'a>(
    pending: &mut Vec<Step<'a>>,
    closer: &'static str,
    depth: usize,
    items: impl DoubleEndedIterator<Item = Vec<Step<'a>>> + ExactSizeIterator,
) {
    pending.push(Step::Text(closer));
    pending.push(Step::Line(depth));
    for (index, steps) in items.enumerate().rev() {
        pending.extend(steps.into_iter().rev());
        pending.push(Step::Line(depth + 1));
        if index > 0 {
            pending.push(Step::Text(","));
        }
    }
}
