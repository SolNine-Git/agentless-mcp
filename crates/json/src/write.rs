//! JSON text in two fixed forms: ASCII-only output and shortest float digits.

use serde::Serialize;
use serde_json::ser::Formatter;
use std::io;

/// The line layout of serialized JSON text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonForm {
    /// One item per line, two spaces of indent per level, `,` after each item but the last.
    Indented,
    /// One line, `, ` between items.
    Compact,
}

/// Serializes `value` in `form`; a non-finite float is written as `null`.
///
/// # Errors
///
/// Returns the serializer's error, for example for a map key that is not a string.
///
/// # Panics
///
/// Panics if the formatter writes a byte outside ASCII, which it never does.
pub fn to_text<T: Serialize + ?Sized>(
    value: &T,
    form: JsonForm,
) -> Result<String, serde_json::Error> {
    let mut buffer = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut buffer, JsonFormatter::new(form));
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(buffer).expect("the formatter writes only ASCII"))
}

struct JsonFormatter {
    separator: &'static [u8],
    line_breaks: bool,
    depth: usize,
    has_value: bool,
}

impl JsonFormatter {
    fn new(form: JsonForm) -> Self {
        let (separator, line_breaks): (&'static [u8], bool) = match form {
            JsonForm::Indented => (b",", true),
            JsonForm::Compact => (b", ", false),
        };
        Self {
            separator,
            line_breaks,
            depth: 0,
            has_value: false,
        }
    }

    fn open<W: ?Sized + io::Write>(&mut self, writer: &mut W, opener: &[u8]) -> io::Result<()> {
        self.depth += 1;
        self.has_value = false;
        writer.write_all(opener)
    }

    // A closed container is a value of its parent, so the parent has a value after it.
    fn close<W: ?Sized + io::Write>(&mut self, writer: &mut W, closer: &[u8]) -> io::Result<()> {
        self.depth -= 1;
        if self.line_breaks && self.has_value {
            self.break_line(writer)?;
        }
        self.has_value = true;
        writer.write_all(closer)
    }

    fn begin_item<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.has_value = true;
        if !first {
            writer.write_all(self.separator)?;
        }
        if self.line_breaks {
            self.break_line(writer)?;
        }
        Ok(())
    }

    fn break_line<W: ?Sized + io::Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b"\n")?;
        for _ in 0..self.depth {
            writer.write_all(b"  ")?;
        }
        Ok(())
    }
}

// The trait has one method per container event, and each forwards to one shared step.
macro_rules! container_events {
    ($($event:ident => $step:ident($delimiter:expr);)*) => {$(
        fn $event<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
            self.$step(writer, $delimiter)
        }
    )*};
}

macro_rules! item_events {
    ($($event:ident),*) => {$(
        fn $event<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
            self.begin_item(writer, first)
        }
    )*};
}

impl Formatter for JsonFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        // serde_json writes a non-finite float as null before it reaches the formatter.
        writer.write_all(crate::float_text(value).as_bytes())
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut units = [0; 2];
        for ch in fragment.chars() {
            if (' '..='~').contains(&ch) {
                writer.write_all(&[ch as u8])?;
                continue;
            }
            for unit in ch.encode_utf16(&mut units) {
                write!(writer, "\\u{unit:04x}")?;
            }
        }
        Ok(())
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }

    container_events! {
        begin_array => open(b"[");
        end_array => close(b"]");
        begin_object => open(b"{");
        end_object => close(b"}");
    }

    item_events!(begin_array_value, begin_object_key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use text::testing::cases;

    #[test]
    fn to_text_matches_fixed_forms() {
        let all: Vec<(String, String, String)> = cases(env!("CARGO_MANIFEST_DIR"), "json", "dumps");
        for (document, expected_indented, expected_compact) in all {
            let value: serde_json::Value =
                serde_json::from_str(&document).expect("the fixture document is JSON");
            assert_eq!(
                to_text(&value, JsonForm::Indented).expect("indenting should succeed"),
                expected_indented
            );
            assert_eq!(
                to_text(&value, JsonForm::Compact).expect("compacting should succeed"),
                expected_compact
            );
        }
    }
}
