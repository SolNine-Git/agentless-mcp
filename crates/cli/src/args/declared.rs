//! The parser of a read operation, built from its declaration in `app::ops::OPS`.

use std::os::unix::ffi::OsStrExt;

use app::IntArg;
use app::ops::{Arity, Cli, Op, Param, Value as Declared};
use text::CodePointString;

use super::{Action, Arg, Convert, Group, Kind, Parser, Value};

const HELP_HELP: &str = "show this help message and exit";

// Returns the `-h`/`--help` action that every parser starts with.
pub(crate) fn help_action() -> Action {
    Action {
        strings: &["-h", "--help"],
        dest: "help",
        metavar: None,
        help: Some(HELP_HELP.to_owned()),
        kind: Kind::Help,
        required: false,
    }
}

pub(crate) fn declared(op: &'static Op) -> Parser {
    let mut actions = vec![help_action()];
    actions.extend(op.params.iter().filter_map(action));
    let group = exclusive(&actions, op.exclusive);
    Parser {
        prog: format!("agentless-mcp {}", op.name),
        description: None,
        actions,
        group,
    }
}

// The command-line action of a parameter, or `None` for a parameter only the MCP tool takes.
pub(crate) fn action(param: &'static Param) -> Option<Action> {
    let help = param.help.map(str::to_owned);
    let action = match param.cli.as_ref()? {
        Cli::Positional { metavar, arity } => Action {
            strings: &[],
            dest: param.dest,
            metavar: Some(metavar),
            help,
            kind: Kind::Positional(*arity),
            required: matches!(arity, Arity::One | Arity::OneOrMore),
        },
        Cli::Option {
            flags,
            metavar,
            repeat,
            required,
        } => Action {
            strings: flags,
            dest: param.dest,
            metavar: *metavar,
            help,
            kind: kind(param.value, *repeat),
            required: *required,
        },
    };
    Some(action)
}

fn kind(value: Declared, repeat: bool) -> Kind {
    let (convert, choices): (Convert, &'static [&'static str]) = match value {
        Declared::Flag | Declared::FlagOrUnset => return Kind::Flag,
        Declared::Text => (TEXT, &[]),
        Declared::Choice(names) => (TEXT, names),
        Declared::Int(_) | Declared::IntOrUnset => (int_value, &[]),
        Declared::Float => (float_value, &[]),
    };
    Kind::Value {
        convert,
        choices,
        repeat,
    }
}

// The mutually exclusive group of the actions named by `dests`, which stand together.
fn exclusive(actions: &[Action], dests: &[&str]) -> Option<Group> {
    let first = actions
        .iter()
        .position(|a| dests.first() == Some(&a.dest))?;
    Some(Group {
        members: first..first + dests.len(),
        required: false,
    })
}

// A text value, kept as given.
pub(crate) const TEXT: Convert = |arg| Ok(Value::Text(arg.clone()));

pub(crate) fn int_value(arg: &Arg) -> Result<Value, String> {
    IntArg::parse(&arg.text)
        .map(Value::Int)
        .ok_or_else(|| invalid("int", arg))
}

// A real number is checked here and kept as its text; the step reads it with `float_text`.
pub(crate) fn float_value(arg: &Arg) -> Result<Value, String> {
    match float_text(&arg.text) {
        Some(_) => Ok(Value::Text(arg.clone())),
        None => Err(invalid("float", arg)),
    }
}

fn invalid(type_name: &str, arg: &Arg) -> String {
    let shown = CodePointString::from_utf8_escaped(arg.raw.as_bytes()).quoted();
    format!("invalid {type_name} value: {shown}")
}

// A real number by the float() text rule: whitespace around it, underscores only between ASCII
// digits, and "inf", "infinity" and "nan" in any case. A digit outside ASCII is refused.
pub(crate) fn float_text(text: &str) -> Option<f64> {
    let core = text::strip(text);
    if !core.is_ascii() {
        return None;
    }
    let bytes = core.as_bytes();
    let between_digits = |at: usize| {
        at > 0
            && bytes[at - 1].is_ascii_digit()
            && bytes.get(at + 1).is_some_and(u8::is_ascii_digit)
    };
    let joined = (0..bytes.len()).all(|at| bytes[at] != b'_' || between_digits(at));
    if !joined {
        return None;
    }
    core.replace('_', "").parse().ok()
}
