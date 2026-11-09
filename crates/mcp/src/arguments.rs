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

    // The integer the call gave, else the tool's own default.
    pub(crate) fn int_or(&self, name: &str, default: usize) -> IntArg {
        self.int(name)
            .cloned()
            .unwrap_or_else(|| IntArg::from_usize(default))
    }

    // A switch: true only when the call gave true.
    pub(crate) fn flag(&self, name: &str) -> bool {
        matches!(self.value(name), Value::Bool(true))
    }
}
