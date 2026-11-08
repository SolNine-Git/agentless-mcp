//! The one JSON object on stdout that hands Claude Code a context string for an event.

use std::io::Write;

use json::JsonValue;

pub(crate) const EMITTED: &str = "emitted";
pub(crate) const INTERNAL: &str = "internal";

// The reason that the run logs: emitted, or internal when stdout does not take the reply.
pub(crate) fn written(stdout: &mut dyn Write, event: &str, context: &str) -> &'static str {
    let reply = JsonValue::Object(vec![(
        "hookSpecificOutput".into(),
        JsonValue::Object(vec![
            ("hookEventName".into(), JsonValue::Str(event.into())),
            ("additionalContext".into(), JsonValue::Str(context.into())),
        ]),
    )]);
    let text = json::value_text(&reply);
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => EMITTED,
        Err(_) => INTERNAL,
    }
}
