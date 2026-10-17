//! Text rules that the tokenizer, the checks and the renderers share: line breaks, whitespace,
//! the printable rule, the word, digit and identifier classes, decimal integer text, the one-line
//! escape, the line diff, the common indentation, shell words, the pure POSIX path rules, and text
//! that can hold a lone surrogate.

mod code_point_string;
mod diff;
mod identifier;
mod indent;
mod integer;
mod lower;
mod posix_path;
mod printable;
mod shell;
mod textsafe;
mod whitespace;
mod word;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use code_point_string::CodePointString;
pub use diff::{matching_blocks, unified_diff};
pub use identifier::is_identifier;
pub use indent::dedent;
pub use integer::{IntText, convertible_int, int_bytes, int_text};
pub use lower::lower;
pub use posix_path::{PosixPath, is_posix_part, posix_anchor, posix_name, split_suffix};
pub use printable::is_printable;
pub use shell::{ShellWordsError, split_shell_words};
pub use textsafe::one_line;
pub use whitespace::{is_ascii_space, is_space, split_whitespace, splitlines, strip, strip_bytes};
pub use word::{decimal_value, is_decimal, is_word};
