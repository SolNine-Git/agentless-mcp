//! Text rules that the tokenizer, the checks and the renderers share: line breaks, whitespace,
//! the printable rule, the word and digit classes, decimal integer text, the one-line escape, and
//! text that can hold a lone surrogate.

mod code_point_string;
mod integer;
mod lower;
mod printable;
mod textsafe;
mod whitespace;
mod word;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use code_point_string::CodePointString;
pub use integer::{IntText, int_bytes, int_text};
pub use lower::lower;
pub use printable::is_printable;
pub use textsafe::one_line;
pub use whitespace::{is_ascii_space, is_space, split_whitespace, splitlines, strip, strip_bytes};
pub use word::{is_decimal, is_word};
