//! Text rules that the tokenizer, the checks and the renderers share: line breaks, whitespace,
//! the printable rule, and text that can hold a lone surrogate.

mod code_point_string;
mod printable;
mod whitespace;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use code_point_string::CodePointString;
pub use printable::is_printable;
pub use whitespace::{is_space, split_whitespace, splitlines, strip};
