//! Text rules that the tokenizer, the checks and the renderers share: line breaks, whitespace,
//! the printable rule, decimal integer text, and text that can hold a lone surrogate.

mod code_point_string;
mod integer;
mod printable;
mod whitespace;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use code_point_string::CodePointString;
pub use integer::{IntText, int_bytes, int_text};
pub use printable::is_printable;
pub use whitespace::{is_ascii_space, is_space, split_whitespace, splitlines, strip, strip_bytes};
