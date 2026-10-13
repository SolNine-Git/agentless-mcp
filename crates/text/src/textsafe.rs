//! The escape that keeps repository text from opening a line in an answer.

use std::borrow::Cow;

/// Returns `text` with each line break and control character written as a visible escape.
#[must_use]
pub fn one_line(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_unsafe) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            _ if is_unsafe(c) => escaped.push_str(&code_escape(c)),
            _ => escaped.push(c),
        }
    }
    Cow::Owned(escaped)
}

// C0 except tab, DEL, C1, and the two Unicode line terminators. A Rust str holds no lone
// surrogate, so the surrogate class of the rule cannot occur here.
fn is_unsafe(c: char) -> bool {
    matches!(
        c,
        '\0'..='\x08' | '\n'..='\x1f' | '\x7f'..='\u{9f}' | '\u{2028}' | '\u{2029}'
    )
}

// Two hex digits up to U+00FF; a wider code with `\x` and four digits would not read back.
fn code_escape(c: char) -> String {
    let code = u32::from(c);
    if code <= 0xff {
        format!("\\x{code:02x}")
    } else {
        format!("\\u{code:04x}")
    }
}
