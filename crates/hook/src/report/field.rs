//! The typed read of one field of a log object, and the integer of any size that a field holds.

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

// The value of `key` in `object` as `read` takes it. A missing key, or a value that `read` does not
// take, is refused with the names of `kinds`.
pub(crate) fn typed<'v, T>(
    object: &'v Pairs,
    key: &str,
    kinds: &[Kind],
    at: &str,
    read: impl Fn(&'v JsonValue) -> Option<T>,
) -> Result<T, String> {
    let Some(value) = json::get(object, key) else {
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
