//! JSON text in two fixed forms, JSON text read into values with the place of each refusal, and
//! the shortest float text.

mod float;
mod parse;
mod write;

pub use float::{float_text, round_half_even};
pub use parse::{JsonError, JsonErrorKind, JsonValue, parse};
pub use write::{JsonForm, to_text};
