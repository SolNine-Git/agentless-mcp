//! The envelope templates.
//! `cargo xtask generate` writes this file from the prompt JSON files.

use crate::template::{Count, Piece, Template, Text};

/// A template that takes (`dropped`, `total`, `max_tokens`).
pub const CEILING_TRUNCATION: Template<(Count, Count, Count)> = Template::new(&[
    Piece::Text("... output truncated at the "),
    Piece::Value(2),
    Piece::Text("-token ceiling: "),
    Piece::Value(0),
    Piece::Text(" of "),
    Piece::Value(1),
    Piece::Text(" lines dropped"),
]);

/// A template that takes `max_tokens`.
pub const JSON_CEILING_OVERSIZED_ITEM: Template<Count> = Template::new(&[
    Piece::Text("payload exceeds the "),
    Piece::Value(0),
    Piece::Text(
        "-token ceiling and the first item alone does not fit; it is \
        returned whole, so this payload is over the ceiling",
    ),
]);

/// A template that takes `max_tokens`.
pub const JSON_CEILING_TRIMMED: Template<Count> = Template::new(&[
    Piece::Text("payload exceeds the "),
    Piece::Value(0),
    Piece::Text("-token ceiling; narrow the request or raise the budget"),
]);

/// A template that takes `max_tokens`.
pub const JSON_CEILING_UNTRIMMABLE: Template<Count> = Template::new(&[
    Piece::Text("payload exceeds the "),
    Piece::Value(0),
    Piece::Text(
        "-token ceiling and cannot be trimmed; request less data or \
        raise the budget",
    ),
]);

/// A text that takes no values.
pub const NOTICE: &str = "repository data below";

/// A template that takes `path`.
pub const RECEIPT_CONFIG: Template<Text> =
    Template::new(&[Piece::Text("// config: "), Piece::Value(0)]);

/// A template that takes `warning`.
pub const RECEIPT_CONFIG_WARNING: Template<Text> =
    Template::new(&[Piece::Text("// config warning: "), Piece::Value(0)]);

/// A text that takes no values.
pub const RECEIPT_HEADER: &str = "// agentless-mcp receipt (repository data below)";

/// A template that takes (`root`, `head`, `dirty`, `cache`).
pub const RECEIPT_LINE: Template<(Text, Text, Text, Text)> = Template::new(&[
    Piece::Text("// repo: "),
    Piece::Value(0),
    Piece::Text("   head: "),
    Piece::Value(1),
    Piece::Text("   dirty: "),
    Piece::Value(2),
    Piece::Text(" files   cache: "),
    Piece::Value(3),
]);

/// A template that takes `note`.
pub const RECEIPT_NOTE: Template<Text> =
    Template::new(&[Piece::Text("// note: "), Piece::Value(0)]);

/// A template that takes `summary`.
pub const RECEIPT_SUMMARY: Template<Text> = Template::new(&[Piece::Text("// "), Piece::Value(0)]);

/// A template that takes (`unit`, `shown`, `total`).
pub const SERVICE_TRUNCATION: Template<(Text, Count, Count)> = Template::new(&[
    Piece::Text("... "),
    Piece::Value(1),
    Piece::Text(" of "),
    Piece::Value(2),
    Piece::Text(" "),
    Piece::Value(0),
    Piece::Text(" shown (narrow the request or raise the budget for the rest)"),
]);
