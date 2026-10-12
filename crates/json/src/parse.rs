//! JSON text read into values, with the message, line and column of each refusal.

use std::collections::HashMap;
use std::fmt;

use text::CodePointString;

// One more level ends the reference decoder with a recursion error (fixture `max_depth`).
const MAX_DEPTH: usize = 9998;

/// A JSON value: integers of any size, `NaN` and infinities, and strings that can hold a lone
/// surrogate.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer of any size, as its decimal text without a sign on zero.
    Int(String),
    /// A number with a fraction or an exponent, or `NaN`, `Infinity` or `-Infinity`.
    Float(f64),
    /// A string. An escape of a lone surrogate keeps that surrogate in it.
    Str(CodePointString),
    /// An array.
    Array(Vec<JsonValue>),
    /// An object: each key once, in first-seen order, with the last value given for it.
    Object(Vec<(CodePointString, JsonValue)>),
}

impl Drop for JsonValue {
    // Moves the children to a heap stack first, so a value nested 9998 deep drops without
    // recursion on the call stack.
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_children(self, &mut pending);
        while let Some(mut child) = pending.pop() {
            take_children(&mut child, &mut pending);
        }
    }
}

fn take_children(value: &mut JsonValue, pending: &mut Vec<JsonValue>) {
    match value {
        JsonValue::Array(items) => pending.append(items),
        JsonValue::Object(pairs) => pending.extend(pairs.drain(..).map(|(_, item)| item)),
        _ => {}
    }
}

/// Why [`parse`] refused a text. It displays as its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonErrorKind {
    /// No value starts at the position.
    ExpectingValue = 0,
    /// An object key does not start with a double quote.
    ExpectingPropertyName = 1,
    /// An object key is not followed by a colon.
    ExpectingColon = 2,
    /// An item is not followed by a comma or by the closing bracket.
    ExpectingComma = 3,
    /// A comma comes right before the closing brace of an object.
    ObjectTrailingComma = 4,
    /// A comma comes right before the closing bracket of an array.
    ArrayTrailingComma = 5,
    /// A string has no closing double quote.
    UnterminatedString = 6,
    /// A string holds a character below U+0020.
    ControlCharacter = 7,
    /// A backslash escape is not one that JSON defines.
    InvalidEscape = 8,
    /// A unicode escape does not have four hex digits.
    InvalidUnicodeEscape = 9,
    /// Text other than whitespace follows the value.
    ExtraData = 10,
    /// The text starts with U+FEFF.
    ByteOrderMark = 11,
    /// Objects nest more than 9998 levels deep.
    ObjectTooDeep = 12,
    /// Arrays nest more than 9998 levels deep.
    ArrayTooDeep = 13,
}

// Indexed by the JsonErrorKind discriminants.
const MESSAGES: [&str; 14] = [
    "Expecting value",
    "Expecting property name enclosed in double quotes",
    "Expecting ':' delimiter",
    "Expecting ',' delimiter",
    "Illegal trailing comma before end of object",
    "Illegal trailing comma before end of array",
    "Unterminated string starting at",
    "Invalid control character at",
    "Invalid \\escape",
    "Invalid \\uXXXX escape",
    "Extra data",
    "Unexpected UTF-8 BOM (decode using utf-8-sig)",
    "maximum recursion depth exceeded while decoding a JSON object from a unicode string",
    "maximum recursion depth exceeded while decoding a JSON array from a unicode string",
];

impl fmt::Display for JsonErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MESSAGES[*self as usize])
    }
}

/// A text that [`parse`] refused: the reason, and where it was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonError {
    /// The reason.
    pub kind: JsonErrorKind,
    /// The position, in code points from the start of the text.
    pub pos: usize,
    /// The 1-based line of the position.
    pub line: usize,
    /// The 1-based column of the position, in code points.
    pub column: usize,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            kind,
            pos,
            line,
            column,
        } = self;
        write!(f, "{kind}: line {line} column {column} (char {pos})")
    }
}

impl std::error::Error for JsonError {}

/// Reads `text` as one JSON value with optional whitespace around it.
///
/// # Errors
///
/// Returns a [`JsonError`] when `text` is not one JSON value.
pub fn parse(text: &str) -> Result<JsonValue, JsonError> {
    let mut reader = Reader { text, at: 0 };
    if text.starts_with('\u{feff}') {
        return Err(reader.error(JsonErrorKind::ByteOrderMark));
    }
    reader.skip_whitespace();
    let value = reader.document()?;
    reader.skip_whitespace();
    if reader.at < text.len() {
        return Err(reader.error(JsonErrorKind::ExtraData));
    }
    Ok(value)
}

const CONSTANTS: [(&str, JsonValue); 6] = [
    ("null", JsonValue::Null),
    ("true", JsonValue::Bool(true)),
    ("false", JsonValue::Bool(false)),
    ("NaN", JsonValue::Float(f64::NAN)),
    ("Infinity", JsonValue::Float(f64::INFINITY)),
    ("-Infinity", JsonValue::Float(f64::NEG_INFINITY)),
];

const SIMPLE_ESCAPES: [(u8, char); 8] = [
    (b'"', '"'),
    (b'\\', '\\'),
    (b'/', '/'),
    (b'b', '\u{8}'),
    (b'f', '\u{c}'),
    (b'n', '\n'),
    (b'r', '\r'),
    (b't', '\t'),
];

struct Reader<'a> {
    text: &'a str,
    // A byte offset. It always sits on a character boundary.
    at: usize,
}

enum Frame {
    Array(Vec<JsonValue>),
    Object(Object),
}

#[derive(Default)]
struct Object {
    pairs: Vec<(CodePointString, JsonValue)>,
    slots: HashMap<CodePointString, usize>,
    key: CodePointString,
}

struct Shape {
    closer: u8,
    trailing_comma: JsonErrorKind,
    too_deep: JsonErrorKind,
}

const ARRAY: Shape = Shape {
    closer: b']',
    trailing_comma: JsonErrorKind::ArrayTrailingComma,
    too_deep: JsonErrorKind::ArrayTooDeep,
};

const OBJECT: Shape = Shape {
    closer: b'}',
    trailing_comma: JsonErrorKind::ObjectTrailingComma,
    too_deep: JsonErrorKind::ObjectTooDeep,
};

impl Frame {
    fn shape(&self) -> Shape {
        match self {
            Self::Array(_) => ARRAY,
            Self::Object(_) => OBJECT,
        }
    }

    fn push(&mut self, value: JsonValue) {
        match self {
            Self::Array(items) => items.push(value),
            Self::Object(object) => object.insert(value),
        }
    }

    fn into_value(self) -> JsonValue {
        match self {
            Self::Array(items) => JsonValue::Array(items),
            Self::Object(object) => JsonValue::Object(object.pairs),
        }
    }
}

impl Object {
    fn insert(&mut self, value: JsonValue) {
        let key = std::mem::take(&mut self.key);
        if let Some(&slot) = self.slots.get(&key) {
            self.pairs[slot].1 = value;
            return;
        }
        self.slots.insert(key.clone(), self.pairs.len());
        self.pairs.push((key, value));
    }
}

impl Reader<'_> {
    // Open containers wait on a heap stack, not on the call stack, so deep nesting costs no stack.
    fn document(&mut self) -> Result<JsonValue, JsonError> {
        let mut open = Vec::new();
        loop {
            let Some(value) = self.value(&mut open)? else {
                continue;
            };
            if let Some(done) = self.close(&mut open, value)? {
                return Ok(done);
            }
        }
    }

    // Returns a complete value, or `None` after it opened a container that is not empty.
    fn value(&mut self, open: &mut Vec<Frame>) -> Result<Option<JsonValue>, JsonError> {
        match self.peek() {
            Some(b'"') => Ok(Some(JsonValue::Str(self.string()?))),
            Some(b'[') => self.open(open, Frame::Array(Vec::new())),
            Some(b'{') => self.open(open, Frame::Object(Object::default())),
            _ => self.scalar().map(Some),
        }
    }

    fn open(
        &mut self,
        open: &mut Vec<Frame>,
        mut frame: Frame,
    ) -> Result<Option<JsonValue>, JsonError> {
        let shape = frame.shape();
        if open.len() >= MAX_DEPTH {
            return Err(self.error(shape.too_deep));
        }
        self.at += 1;
        self.skip_whitespace();
        if self.peek() == Some(shape.closer) {
            self.at += 1;
            return Ok(Some(frame.into_value()));
        }
        self.item_prefix(&mut frame)?;
        open.push(frame);
        Ok(None)
    }

    // Adds `value` to the open containers, and returns the document when the last one closes.
    fn close(
        &mut self,
        open: &mut Vec<Frame>,
        mut value: JsonValue,
    ) -> Result<Option<JsonValue>, JsonError> {
        while let Some(mut frame) = open.pop() {
            if self.add(&mut frame, value)? {
                value = frame.into_value();
                continue;
            }
            open.push(frame);
            return Ok(None);
        }
        Ok(Some(value))
    }

    // Returns true when the container closed after `value`, false when another item follows.
    fn add(&mut self, frame: &mut Frame, value: JsonValue) -> Result<bool, JsonError> {
        frame.push(value);
        self.skip_whitespace();
        let shape = frame.shape();
        match self.peek() {
            Some(byte) if byte == shape.closer => {
                self.at += 1;
                return Ok(true);
            }
            Some(b',') => {}
            _ => return Err(self.error(JsonErrorKind::ExpectingComma)),
        }
        let comma = self.at;
        self.at += 1;
        self.skip_whitespace();
        if self.peek() == Some(shape.closer) {
            return Err(self.error_at(shape.trailing_comma, comma));
        }
        self.item_prefix(frame)?;
        Ok(false)
    }

    // An object item starts with its key and a colon. An array item has no prefix.
    fn item_prefix(&mut self, frame: &mut Frame) -> Result<(), JsonError> {
        if let Frame::Object(object) = frame {
            self.require(b'"', JsonErrorKind::ExpectingPropertyName)?;
            object.key = self.string()?;
            self.skip_whitespace();
            self.require(b':', JsonErrorKind::ExpectingColon)?;
            self.at += 1;
            self.skip_whitespace();
        }
        Ok(())
    }

    fn scalar(&mut self) -> Result<JsonValue, JsonError> {
        let rest = &self.text[self.at..];
        // No constant starts with a digit or with '-' and a digit, so a number skips the scan.
        if let [b'0'..=b'9', ..] | [b'-', b'0'..=b'9', ..] = rest.as_bytes() {
            return self.number();
        }
        for (word, value) in CONSTANTS {
            if rest.starts_with(word) {
                self.at += word.len();
                return Ok(value);
            }
        }
        self.number()
    }

    fn number(&mut self) -> Result<JsonValue, JsonError> {
        let bytes = self.text.as_bytes();
        let start = self.at;
        let mut end = start + usize::from(bytes.get(start) == Some(&b'-'));
        match bytes.get(end) {
            Some(b'1'..=b'9') => end = digits_end(bytes, end + 1),
            Some(b'0') => end += 1,
            _ => return Err(self.error(JsonErrorKind::ExpectingValue)),
        }
        let mut is_float = false;
        if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
            is_float = true;
            end = digits_end(bytes, end + 2);
        }
        if let Some(exponent_end) = exponent_end(bytes, end) {
            is_float = true;
            end = exponent_end;
        }
        let literal = &self.text[start..end];
        self.at = end;
        if !is_float {
            // JSON allows no leading zero, so -0 is the one integer whose text changes.
            let digits = if literal == "-0" { "0" } else { literal };
            return Ok(JsonValue::Int(digits.to_owned()));
        }
        literal
            .parse()
            .map(JsonValue::Float)
            .map_err(|_| self.error_at(JsonErrorKind::ExpectingValue, start))
    }

    // Reads the string whose opening quote is at `self.at`.
    fn string(&mut self) -> Result<CodePointString, JsonError> {
        let begin = self.at;
        self.at += 1;
        let mut text = CodePointString::default();
        loop {
            let stop = self.chunk_end(begin)?;
            text.push_str(&self.text[self.at..stop]);
            self.at = stop + 1;
            if self.text.as_bytes()[stop] == b'"' {
                return Ok(text);
            }
            self.escape(begin, &mut text)?;
        }
    }

    // Returns the offset of the next double quote or backslash.
    fn chunk_end(&self, begin: usize) -> Result<usize, JsonError> {
        for (offset, &byte) in self.text.as_bytes()[self.at..].iter().enumerate() {
            match byte {
                b'"' | b'\\' => return Ok(self.at + offset),
                0x00..=0x1f => {
                    return Err(self.error_at(JsonErrorKind::ControlCharacter, self.at + offset));
                }
                _ => {}
            }
        }
        Err(self.error_at(JsonErrorKind::UnterminatedString, begin))
    }

    // Reads the escape after the backslash at `self.at - 1`.
    fn escape(&mut self, begin: usize, text: &mut CodePointString) -> Result<(), JsonError> {
        let Some(&code) = self.text.as_bytes().get(self.at) else {
            return Err(self.error_at(JsonErrorKind::UnterminatedString, begin));
        };
        if code == b'u' {
            return self.unicode_escape(text);
        }
        let Some(&(_, unescaped)) = SIMPLE_ESCAPES.iter().find(|&&(name, _)| name == code) else {
            return Err(self.error_at(JsonErrorKind::InvalidEscape, self.at - 1));
        };
        text.push(u32::from(unescaped));
        self.at += 1;
        Ok(())
    }

    // Reads the unicode escape whose `u` is at `self.at`, and joins a surrogate pair of escapes.
    fn unicode_escape(&mut self, text: &mut CodePointString) -> Result<(), JsonError> {
        let u_at = self.at;
        let mut end = u_at + 5;
        let invalid = |at| self.error_at(JsonErrorKind::InvalidUnicodeEscape, at);
        if end >= self.text.len() {
            return Err(invalid(u_at));
        }
        let mut code = self.hex4(u_at + 1).ok_or_else(|| invalid(u_at))?;
        let bytes = self.text.as_bytes();
        let low_follows = end + 6 < bytes.len() && bytes[end..].starts_with(b"\\u");
        if (0xd800..=0xdbff).contains(&code) && low_follows {
            let low = self.hex4(end + 2).ok_or_else(|| invalid(end + 1))?;
            if (0xdc00..=0xdfff).contains(&low) {
                code = 0x1_0000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                end += 6;
            }
        }
        text.push(code);
        self.at = end;
        Ok(())
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let digits = self.text.as_bytes().get(at..at + 4)?;
        digits.iter().try_fold(0, |code, &digit| {
            Some((code << 4) | char::from(digit).to_digit(16)?)
        })
    }

    fn require(&self, byte: u8, kind: JsonErrorKind) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            Ok(())
        } else {
            Err(self.error(kind))
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn error(&self, kind: JsonErrorKind) -> JsonError {
        self.error_at(kind, self.at)
    }

    fn error_at(&self, kind: JsonErrorKind, at: usize) -> JsonError {
        let before = &self.text[..at];
        let pos = before.chars().count();
        let line = before.matches('\n').count() + 1;
        let column = before
            .rfind('\n')
            .map_or(pos + 1, |newline| before[newline..].chars().count());
        JsonError {
            kind,
            pos,
            line,
            column,
        }
    }
}

fn digits_end(bytes: &[u8], from: usize) -> usize {
    from + bytes[from..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
}

// An exponent counts only with at least one digit; without one, the number ends before the `e`.
fn exponent_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at + 1 >= bytes.len() || !matches!(bytes[at], b'e' | b'E') {
        return None;
    }
    let mut end = at + 1;
    if end + 1 < bytes.len() && matches!(bytes[end], b'+' | b'-') {
        end += 1;
    }
    let end = digits_end(bytes, end);
    bytes[end - 1].is_ascii_digit().then_some(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use text::testing::cases;

    fn code_points(text: &CodePointString) -> Vec<u32> {
        text.code_points().collect()
    }

    fn tagged(value: &JsonValue) -> Value {
        match value {
            JsonValue::Null => json!(["null"]),
            JsonValue::Bool(flag) => json!(["bool", flag]),
            JsonValue::Int(digits) => json!(["int", digits]),
            JsonValue::Float(number) => json!(["float", format!("0x{:016x}", number.to_bits())]),
            JsonValue::Str(text) => json!(["str", code_points(text)]),
            JsonValue::Array(items) => {
                json!(["array", items.iter().map(tagged).collect::<Vec<_>>()])
            }
            JsonValue::Object(pairs) => {
                let pairs: Vec<Value> = pairs
                    .iter()
                    .map(|(key, item)| json!([code_points(key), tagged(item)]))
                    .collect();
                json!(["object", pairs])
            }
        }
    }

    #[test]
    fn parse_cases() {
        let all: Vec<(String, Value)> = cases(env!("CARGO_MANIFEST_DIR"), "json", "loads");
        for (document, expected) in all {
            let found = match parse(&document) {
                Ok(value) => json!(["ok", tagged(&value)]),
                Err(error) => {
                    json!([
                        "error",
                        error.kind.to_string(),
                        error.pos,
                        error.line,
                        error.column
                    ])
                }
            };
            assert_eq!(found, expected, "document: {document:?}");
        }
    }

    #[test]
    fn nesting_one_level_past_the_limit_is_refused() {
        let [limit]: [usize; 1] = cases(env!("CARGO_MANIFEST_DIR"), "json", "max_depth");
        assert_eq!(MAX_DEPTH, limit);
        for (opener, middle, closer, kind) in [
            ("[", "", "]", JsonErrorKind::ArrayTooDeep),
            ("{\"a\": ", "0", "}", JsonErrorKind::ObjectTooDeep),
        ] {
            let nested =
                |depth| format!("{}{middle}{}", opener.repeat(depth), closer.repeat(depth));
            assert!(
                parse(&nested(limit)).is_ok(),
                "{opener} nested {limit} deep"
            );
            let refused = parse(&nested(limit + 1)).map(drop);
            assert_eq!(refused.map_err(|error| error.kind), Err(kind));
        }
    }

    #[test]
    fn an_error_displays_its_message_line_column_and_position() {
        let error = parse("{\"a\":1\n\n,\"b\"}").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Expecting ':' delimiter: line 3 column 5 (char 12)"
        );
    }
}
