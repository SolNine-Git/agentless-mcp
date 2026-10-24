//! Text as a sequence of code points, where a lone surrogate can occur.

use crate::is_printable;

/// Text as a sequence of code points, where a lone surrogate (U+D800 to U+DFFF) can occur.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodePointString {
    // Generalized UTF-8: a lone surrogate takes the 3-byte form of its code point, so the byte
    // order is the code point order.
    bytes: Vec<u8>,
}

impl CodePointString {
    /// Decodes `bytes` as UTF-8: a byte that is not part of valid UTF-8 becomes the code point
    /// U+DC00 plus that byte.
    #[must_use]
    pub fn from_utf8_escaped(bytes: &[u8]) -> Self {
        let mut text = Self::default();
        for chunk in bytes.utf8_chunks() {
            text.push_str(chunk.valid());
            for &byte in chunk.invalid() {
                text.push(0xdc00 + u32::from(byte));
            }
        }
        text
    }

    /// Returns the text as a `str`, or `None` when it holds a lone surrogate.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// Returns the code points in order.
    pub fn code_points(&self) -> impl Iterator<Item = u32> + '_ {
        let mut bytes = self.bytes.iter().copied();
        std::iter::from_fn(move || {
            let lead = bytes.next()?;
            let (mut code, tail) = match lead {
                0x00..=0x7f => return Some(u32::from(lead)),
                0xc0..=0xdf => (u32::from(lead & 0x1f), 1),
                0xe0..=0xef => (u32::from(lead & 0x0f), 2),
                _ => (u32::from(lead & 0x07), 3),
            };
            for byte in bytes.by_ref().take(tail) {
                code = (code << 6) | u32::from(byte & 0x3f);
            }
            Some(code)
        })
    }

    /// Returns the text quoted, with each quote, backslash and character that is not printable
    /// written as an escape.
    #[must_use]
    pub fn quoted(&self) -> String {
        let has = |wanted: char| self.code_points().any(|code| code == u32::from(wanted));
        let quote = if has('\'') && !has('"') { '"' } else { '\'' };
        let mut out = String::with_capacity(self.bytes.len() + 2);
        out.push(quote);
        for code in self.code_points() {
            push_quoted(&mut out, code, quote);
        }
        out.push(quote);
        out
    }

    /// Appends the code point `code`, which can be a lone surrogate.
    ///
    /// # Panics
    ///
    /// Panics when `code` is past U+10FFFF, the last code point.
    pub fn push(&mut self, code: u32) {
        assert!(code <= 0x10_ffff, "{code:#x} is past the last code point");
        if let Some(c) = char::from_u32(code) {
            self.push_str(c.encode_utf8(&mut [0; 4]));
            return;
        }
        // Each other value up to U+10FFFF is a lone surrogate: the three bytes that the UTF-8
        // pattern gives its code point.
        let [low, high, ..] = code.to_le_bytes();
        self.bytes.extend([
            0xe0 | (high >> 4),
            0x80 | ((high & 0x0f) << 2) | (low >> 6),
            0x80 | (low & 0x3f),
        ]);
    }

    /// Appends `text`.
    pub fn push_str(&mut self, text: &str) {
        self.bytes.extend_from_slice(text.as_bytes());
    }
}

impl From<&str> for CodePointString {
    fn from(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
        }
    }
}

fn push_quoted(out: &mut String, code: u32, quote: char) {
    match char::from_u32(code) {
        Some(c) if c == quote || c == '\\' => {
            out.push('\\');
            out.push(c);
        }
        Some('\t') => out.push_str("\\t"),
        Some('\n') => out.push_str("\\n"),
        Some('\r') => out.push_str("\\r"),
        Some(c) if is_copied(c) => out.push(c),
        _ => push_escape(out, code),
    }
}

fn is_copied(c: char) -> bool {
    if c.is_ascii() {
        (' '..='~').contains(&c)
    } else {
        is_printable(u32::from(c))
    }
}

fn push_escape(out: &mut String, code: u32) {
    let escape = match code {
        0x00..=0xff => format!("\\x{code:02x}"),
        0x100..=0xffff => format!("\\u{code:04x}"),
        _ => format!("\\U{code:08x}"),
    };
    out.push_str(&escape);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::cases;

    fn from_code_points(codes: &[u32]) -> CodePointString {
        let mut text = CodePointString::default();
        for &code in codes {
            text.push(code);
        }
        text
    }

    #[test]
    fn quoted_cases() {
        let all: Vec<(Vec<u32>, String)> = cases(env!("CARGO_MANIFEST_DIR"), "text", "repr");
        for (codes, expected) in all {
            let text = from_code_points(&codes);
            assert_eq!(text.code_points().collect::<Vec<_>>(), codes);
            assert_eq!(text.quoted(), expected, "code points: {codes:x?}");
        }
    }

    #[test]
    fn from_utf8_escaped_cases() {
        let all: Vec<(Vec<u8>, Vec<u32>)> = cases(env!("CARGO_MANIFEST_DIR"), "text", "fsdecode");
        for (bytes, expected) in all {
            let decoded: Vec<u32> = CodePointString::from_utf8_escaped(&bytes)
                .code_points()
                .collect();
            assert_eq!(decoded, expected, "bytes: {bytes:x?}");
        }
    }

    #[test]
    #[should_panic(expected = "0x110000 is past the last code point")]
    fn push_refuses_a_value_above_the_last_code_point() {
        CodePointString::default().push(0x11_0000);
    }

    #[test]
    fn push_takes_the_last_code_point() {
        let text = from_code_points(&[0x10_ffff]);
        assert_eq!(text.code_points().collect::<Vec<_>>(), [0x10_ffff]);
    }

    #[test]
    fn a_lone_surrogate_has_no_str_form_and_sorts_by_code_point() {
        let surrogate = from_code_points(&[0xd800]);
        assert_eq!(surrogate.as_str(), None);
        assert_eq!(CodePointString::from("ab").as_str(), Some("ab"));
        let mut sorted = [
            from_code_points(&[0xe000]),
            surrogate.clone(),
            from_code_points(&[0xd7ff]),
        ];
        sorted.sort();
        let firsts: Vec<u32> = sorted
            .iter()
            .flat_map(CodePointString::code_points)
            .collect();
        assert_eq!(firsts, [0xd7ff, 0xd800, 0xe000]);
    }
}
