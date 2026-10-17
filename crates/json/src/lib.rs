//! JSON text in three fixed forms, JSON text read into values with the place of each refusal and
//! those values written back as text, the shortest float text, and the fixed-point and general
//! float texts.

mod float;
mod parse;
mod write;

pub use float::{fixed, float_text, general, round_half_even};
pub use parse::{JsonError, JsonErrorKind, JsonValue, parse};
pub use write::{JsonForm, to_text, value_text};
