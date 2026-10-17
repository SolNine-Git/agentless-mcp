//! The typed read of one field of a log object, the integer of any size that a field holds, and
//! the reference's display form of a JSON value.

use std::cmp::Ordering;
use std::fmt;

use ::text::CodePointString;
use json::JsonValue;

// The JSON types a field may hold, named as the reference names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Str,
    Int,
    Float,
    Dict,
    List,
    Null,
}

impl Kind {
    fn name(self) -> &'static str {
        ["str", "int", "float", "dict", "list", "null"][self as usize]
    }
}

pub(crate) type Pairs = [(CodePointString, JsonValue)];

// The value of `key` in `object`, or `None` when the object has no such key.
pub(crate) fn entry<'v>(object: &'v Pairs, key: &str) -> Option<&'v JsonValue> {
    object
        .iter()
        .find(|(name, _)| name.as_str() == Some(key))
        .map(|(_, value)| value)
}

// The value of `key` in `object` as `read` takes it. A missing key, or a value that `read` does not
// take, is refused with the names of `kinds`.
pub(crate) fn typed<'v, T>(
    object: &'v Pairs,
    key: &str,
    kinds: &[Kind],
    at: &str,
    read: impl Fn(&'v JsonValue) -> Option<T>,
) -> Result<T, String> {
    let Some(value) = entry(object, key) else {
        return Err(format!("{at}: {key} is missing"));
    };
    read(value).ok_or_else(|| {
        let names: Vec<&str> = kinds.iter().map(|kind| kind.name()).collect();
        format!("{at}: {key} is not {}", names.join(" or "))
    })
}

pub(crate) fn text(object: &Pairs, key: &str, at: &str) -> Result<CodePointString, String> {
    typed(object, key, &[Kind::Str], at, |value| {
        seen(value).text.cloned()
    })
}

pub(crate) fn text_or_null(
    object: &Pairs,
    key: &str,
    at: &str,
) -> Result<Option<CodePointString>, String> {
    let read = |value| or_null(value, |value| seen(value).text.cloned());
    typed(object, key, &[Kind::Str, Kind::Null], at, read).map(|held| held.0)
}

pub(crate) fn int(object: &Pairs, key: &str, at: &str) -> Result<Int, String> {
    typed(object, key, &[Kind::Int], at, |value| seen(value).int)
}

pub(crate) fn int_or_null(object: &Pairs, key: &str, at: &str) -> Result<Option<Int>, String> {
    let read = |value| or_null(value, |value| seen(value).int);
    typed(object, key, &[Kind::Int, Kind::Null], at, read).map(|held| held.0)
}

pub(crate) fn list<'v>(object: &'v Pairs, key: &str, at: &str) -> Result<&'v [JsonValue], String> {
    typed(object, key, &[Kind::List], at, |value| seen(value).list)
}

pub(crate) fn dict_or_null<'v>(
    object: &'v Pairs,
    key: &str,
    at: &str,
) -> Result<Option<&'v Pairs>, String> {
    let read = |value| or_null(value, |value| seen(value).dict);
    typed(object, key, &[Kind::Dict, Kind::Null], at, read).map(|held| held.0)
}

// A value that may be null: `None` for a null.
struct Nullable<T>(Option<T>);

// A null, or another value as `read` takes it.
fn or_null<'v, T>(
    value: &'v JsonValue,
    read: impl Fn(&'v JsonValue) -> Option<T>,
) -> Option<Nullable<T>> {
    match value {
        JsonValue::Null => Some(Nullable(None)),
        _ => read(value).map(|held| Nullable(Some(held))),
    }
}

// A value seen as each type that the reader takes. At most one view is set.
#[derive(Default)]
pub(crate) struct Seen<'v> {
    pub(crate) text: Option<&'v CodePointString>,
    pub(crate) int: Option<Int>,
    pub(crate) dict: Option<&'v Pairs>,
    pub(crate) list: Option<&'v [JsonValue]>,
}

pub(crate) fn seen(value: &JsonValue) -> Seen<'_> {
    match value {
        JsonValue::Str(text) => Seen {
            text: Some(text),
            ..Seen::default()
        },
        JsonValue::Int(digits) => Seen {
            int: Some(Int(digits.clone())),
            ..Seen::default()
        },
        JsonValue::Object(pairs) => Seen {
            dict: Some(pairs),
            ..Seen::default()
        },
        JsonValue::Array(items) => Seen {
            list: Some(items),
            ..Seen::default()
        },
        _ => Seen::default(),
    }
}

// An integer of any size, as its decimal text: a sign only on a negative value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Int(String);

impl Int {
    fn magnitude(&self) -> &str {
        self.0.strip_prefix('-').unwrap_or(&self.0)
    }

    fn negative(&self) -> bool {
        self.0.starts_with('-')
    }

    // The nearest float, or `None` past the float range.
    pub(crate) fn to_float(&self) -> Option<f64> {
        let value: f64 = self.0.parse().ok()?;
        value.is_finite().then_some(value)
    }
}

impl Ord for Int {
    fn cmp(&self, other: &Self) -> Ordering {
        let by_magnitude = || {
            let (a, b) = (self.magnitude(), other.magnitude());
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        };
        match (self.negative(), other.negative()) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => by_magnitude(),
            (true, true) => by_magnitude().reverse(),
        }
    }
}

impl PartialOrd for Int {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

enum Piece<'v> {
    Value(&'v JsonValue),
    Key(&'v CodePointString),
    Text(&'static str),
}

// The reference's display form of a parsed JSON value: None, True, quoted strings, float repr.
pub(crate) fn py_repr(value: &JsonValue) -> String {
    let mut out = String::new();
    let mut pending = vec![Piece::Value(value)];
    while let Some(piece) = pending.pop() {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Key(key) => out.push_str(&key.quoted()),
            Piece::Value(value) => open(value, &mut out, &mut pending),
        }
    }
    out
}

fn open<'v>(value: &'v JsonValue, out: &mut String, pending: &mut Vec<Piece<'v>>) {
    match value {
        JsonValue::Null => out.push_str("None"),
        JsonValue::Bool(true) => out.push_str("True"),
        JsonValue::Bool(false) => out.push_str("False"),
        JsonValue::Int(digits) => out.push_str(digits),
        JsonValue::Float(x) => out.push_str(&json::float_text(*x)),
        JsonValue::Str(text) => out.push_str(&text.quoted()),
        JsonValue::Array(items) => {
            out.push('[');
            pending.push(Piece::Text("]"));
            let items = items.iter().map(|item| vec![Piece::Value(item)]);
            push_items(pending, items.collect());
        }
        JsonValue::Object(pairs) => {
            out.push('{');
            pending.push(Piece::Text("}"));
            let pairs = pairs
                .iter()
                .map(|(key, item)| vec![Piece::Key(key), Piece::Text(": "), Piece::Value(item)]);
            push_items(pending, pairs.collect());
        }
    }
}

// Queues the items to be written in order, with ", " between them.
fn push_items<'v>(pending: &mut Vec<Piece<'v>>, items: Vec<Vec<Piece<'v>>>) {
    for (index, item) in items.into_iter().enumerate().rev() {
        pending.extend(item.into_iter().rev());
        if index > 0 {
            pending.push(Piece::Text(", "));
        }
    }
}
