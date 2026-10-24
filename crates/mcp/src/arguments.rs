//! The validated arguments of one call: each parameter of the tool, in declaration order, with its
//! value or null.

use app::IntArg;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Int(IntArg),
    Float(f64),
    Text(String),
    List(Vec<Value>),
}

// Each parameter of a tool with its validated value; an absent parameter holds its default.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Arguments {
    values: Vec<(&'static str, Value)>,
}

impl Arguments {
    pub(crate) fn new(values: Vec<(&'static str, Value)>) -> Arguments {
        Arguments { values }
    }

    // Each parameter and its value, in declaration order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &(&'static str, Value)> {
        self.values.iter()
    }

    pub(crate) fn value(&self, name: &str) -> &Value {
        let found = self.values.iter().find(|(param, _)| *param == name);
        found.map_or(&Value::Null, |(_, value)| value)
    }

    pub(crate) fn text(&self, name: &str) -> Option<&str> {
        match self.value(name) {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) fn int(&self, name: &str) -> Option<&IntArg> {
        match self.value(name) {
            Value::Int(value) => Some(value),
            _ => None,
        }
    }

    // The integer the call gave, else the operation's own default.
    pub(crate) fn int_or(&self, name: &str, default: usize) -> IntArg {
        self.int(name)
            .cloned()
            .unwrap_or_else(|| IntArg::from_usize(default))
    }

    // A text the validation requires, so every call holds one.
    pub(crate) fn required(&self, name: &str) -> &str {
        self.text(name)
            .unwrap_or_else(|| panic!("the validation requires the text {name}"))
    }

    // An optional text where null means the same as an empty text to the operation.
    pub(crate) fn text_or_empty(&self, name: &str) -> String {
        self.text(name).map_or_else(String::new, str::to_owned)
    }

    pub(crate) fn float(&self, name: &str) -> Option<f64> {
        match self.value(name) {
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }

    // A switch: true only when the call gave true.
    pub(crate) fn flag(&self, name: &str) -> bool {
        matches!(self.value(name), Value::Bool(true))
    }

    // True or false as the call gave it, or `None` for null.
    pub(crate) fn switch(&self, name: &str) -> Option<bool> {
        let given = self.flag(name) || matches!(self.value(name), Value::Bool(false));
        given.then(|| self.flag(name))
    }

    // The texts of a list; null reads as no entries, as an empty list does.
    pub(crate) fn texts(&self, name: &str) -> Vec<String> {
        let Value::List(items) = self.value(name) else {
            return Vec::new();
        };
        let texts = items.iter().filter_map(|item| match item {
            Value::Text(text) => Some(text.clone()),
            _ => None,
        });
        texts.collect()
    }
}
