//! The command-line parser: option matching, positional arities, values, conflicts, and the usage
//! and help texts.

mod declared;
mod help;
mod nargs;
mod parse;
#[cfg(test)]
mod tests;
mod wrap;

use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::os::unix::ffi::OsStrExt;

use app::IntArg;
use app::ops::{Arity, Op};

pub(crate) use declared::{TEXT, action, float_text, float_value, help_action, int_value};
pub(crate) use parse::{error, parse, parse_alone, parse_alone_checked};

// One command-line argument: its text for matching and messages, and its bytes for paths.
#[derive(Clone, Debug)]
pub(crate) struct Arg {
    pub(crate) text: String,
    pub(crate) raw: OsString,
}

impl Arg {
    pub(crate) fn new(raw: &OsStr) -> Arg {
        Arg {
            text: raw.to_string_lossy().into_owned(),
            raw: raw.to_owned(),
        }
    }

    // The text of an argument that a command reads as text, or the refusal of one that is not
    // UTF-8 (ledger deviation), which names the argument as `name`.
    pub(crate) fn utf8(&self, name: &str) -> Result<String, String> {
        let text = self.raw.to_str().map(str::to_owned);
        text.ok_or_else(|| format!("{name} is not valid UTF-8: {}", self.text))
    }

    // The part after the first `=`, as the value of a `--name=value` argument.
    fn after_equals(&self) -> Arg {
        let bytes = self.raw.as_bytes();
        let start = bytes
            .iter()
            .position(|&b| b == b'=')
            .map_or(bytes.len(), |at| at + 1);
        Arg::new(OsStr::from_bytes(&bytes[start..]))
    }
}

pub(crate) struct Parser {
    pub(crate) prog: String,
    pub(crate) description: Option<&'static str>,
    pub(crate) actions: Vec<Action>,
    pub(crate) group: Option<Group>,
}

impl Parser {
    // Each option string with its action, in declaration order.
    fn option_strings(&self) -> impl Iterator<Item = (usize, &'static str)> + '_ {
        let actions = self.actions.iter().enumerate();
        actions.flat_map(|(index, action)| action.strings.iter().map(move |s| (index, *s)))
    }

    fn option(&self, text: &str) -> Option<usize> {
        self.option_strings()
            .find(|(_, s)| *s == text)
            .map(|(index, _)| index)
    }

    // The other members of the mutually exclusive group of `index`, in group order.
    fn conflicts(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        let members = self
            .group
            .as_ref()
            .map_or(0..0, |group| group.members.clone());
        let grouped = members.contains(&index);
        members.filter(move |&other| grouped && other != index)
    }
}

// A mutually exclusive group: consecutive actions, of which a run may name at most one.
pub(crate) struct Group {
    pub(crate) members: Range<usize>,
    pub(crate) required: bool,
}

pub(crate) struct Action {
    pub(crate) strings: &'static [&'static str],
    pub(crate) dest: &'static str,
    pub(crate) metavar: Option<&'static str>,
    pub(crate) help: Option<String>,
    pub(crate) kind: Kind,
    pub(crate) required: bool,
}

impl Action {
    // The name an error message gives the action.
    fn name(&self) -> String {
        if self.strings.is_empty() {
            self.metavar.unwrap_or(self.dest).to_owned()
        } else {
            self.strings.join("/")
        }
    }
}

pub(crate) enum Kind {
    Help,
    Flag,
    Value {
        convert: Convert,
        choices: &'static [&'static str],
        repeat: bool,
    },
    Positional(Arity),
    Commands(&'static [Command]),
    // Prints its text, as the help action prints the help, and ends the parse.
    Version(&'static str),
}

// Turns an option's text into its value, or gives the message of an argument error.
pub(crate) type Convert = fn(&Arg) -> Result<Value, String>;

// A further conversion of a converted value, by the destination of its action. A parse runs it at
// the action's place in the command line, so its refusal is an argument error in order.
pub(crate) type Check<'c> = dyn FnMut(&'static str, Value) -> Result<Value, String> + 'c;

pub(crate) struct Command {
    pub(crate) name: &'static str,
    pub(crate) help: &'static str,
    pub(crate) form: Form,
}

impl Command {
    pub(crate) fn parser(&self) -> Parser {
        match self.form {
            Form::Built(build) => build(),
            Form::Read(op) => declared::declared(op),
        }
    }
}

// How a subcommand's parser is built: by hand, or from its read operation.
pub(crate) enum Form {
    Built(fn() -> Parser),
    Read(&'static Op),
}

#[derive(Clone, Debug)]
pub(crate) enum Value {
    Flag,
    Text(Arg),
    Names(Vec<String>),
    List(Vec<Arg>),
    Int(IntArg),
}

// The values that one parser's run set, by destination; a repeated option keeps its last value,
// and an appending option keeps every value.
#[derive(Debug, Default)]
pub(crate) struct Namespace {
    values: Vec<(&'static str, Value)>,
}

impl Namespace {
    fn set(&mut self, dest: &'static str, value: Value) {
        self.values.retain(|(name, _)| *name != dest);
        self.values.push((dest, value));
    }

    fn append(&mut self, dest: &'static str, arg: Arg) {
        let mut list = self.list(dest).to_vec();
        list.push(arg);
        self.set(dest, Value::List(list));
    }

    // The value set under `dest`, or `None` when the run set none.
    pub(crate) fn get(&self, dest: &str) -> Option<&Value> {
        self.values
            .iter()
            .find(|(name, _)| *name == dest)
            .map(|(_, value)| value)
    }

    pub(crate) fn flag(&self, dest: &str) -> bool {
        matches!(self.get(dest), Some(Value::Flag))
    }

    pub(crate) fn text(&self, dest: &str) -> Option<&Arg> {
        match self.get(dest) {
            Some(Value::Text(arg)) => Some(arg),
            _ => None,
        }
    }

    pub(crate) fn names(&self, dest: &str) -> Option<&[String]> {
        match self.get(dest) {
            Some(Value::Names(names)) => Some(names),
            _ => None,
        }
    }

    // Every value of a list: an appending option's uses, or a positional's arguments.
    pub(crate) fn list(&self, dest: &str) -> &[Arg] {
        match self.get(dest) {
            Some(Value::List(list)) => list,
            _ => &[],
        }
    }
}

// The command a parse chose. A hand-built command names its path from the top, as
// ["patch", "parse"], with the values of the last parser on it.
pub(crate) enum Chosen {
    Ported {
        path: Vec<&'static str>,
        namespace: Namespace,
    },
    Read {
        op: &'static Op,
        namespace: Namespace,
    },
}

pub(crate) struct Parsed {
    pub(crate) top: Namespace,
    pub(crate) chosen: Chosen,
}

// A parse that ends the run: the help text for stdout, or a usage error for stderr.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    Help(String),
    Usage(String),
}
