//! The read operations, each declared once: its command-line form, the MCP names of its
//! parameters, its defaults and its help text.

pub mod limits;
mod table;

pub use table::{JSON, NO_CACHE, OPS, REPO};

/// One read operation.
#[derive(Debug)]
pub struct Op {
    /// The subcommand name.
    pub name: &'static str,
    /// The subcommand's one-line help.
    pub summary: &'static str,
    /// The MCP tool that answers the same question, or `None` for a command-line-only operation.
    pub tool: Option<&'static str>,
    /// The parameters, in declaration order.
    pub params: &'static [Param],
    /// The destinations of the one mutually exclusive group, or none.
    pub exclusive: &'static [&'static str],
}

/// One parameter of a read operation.
#[derive(Debug)]
pub struct Param {
    /// The name the parsed value goes under.
    pub dest: &'static str,
    /// The command-line form, or `None` when only the MCP tool takes it.
    pub cli: Option<Cli>,
    /// How the MCP tool spells it.
    pub door: Door,
    /// The value's type and default.
    pub value: Value,
    /// The help text, or `None` for an entry without one.
    pub help: Option<&'static str>,
}

/// The command-line form of a parameter.
#[derive(Debug)]
pub enum Cli {
    /// A positional argument.
    Positional {
        /// The name the usage line shows.
        metavar: &'static str,
        /// How many arguments it takes.
        arity: Arity,
    },
    /// An option.
    Option {
        /// The option strings.
        flags: &'static [&'static str],
        /// The value's name in the usage line, or `None` for the default name.
        metavar: Option<&'static str>,
        /// True when each use appends to a list.
        repeat: bool,
        /// True when the command line must name it.
        required: bool,
    },
}

/// How many arguments a positional parameter takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arity {
    /// Exactly one.
    One,
    /// None or one.
    Optional,
    /// One or more.
    OneOrMore,
    /// Any number.
    Any,
}

/// How the MCP tool spells a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    /// The tool takes it under the same name.
    Same,
    /// The tool takes it under another name.
    Named(&'static str),
    /// Only the command line takes it.
    CliOnly,
}

/// The type and the default of a parameter's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A switch that is false unless given.
    Flag,
    /// A switch that is unset unless given, so the config can decide.
    FlagOrUnset,
    /// A text, unset unless given.
    Text,
    /// One of the listed names, unset unless given.
    Choice(&'static [&'static str]),
    /// An integer with this default.
    Int(usize),
    /// An integer, unset unless given.
    IntOrUnset,
    /// A real number, unset unless given.
    Float,
}

/// Returns the read operation with the subcommand name `name`.
#[must_use]
pub fn op(name: &str) -> Option<&'static Op> {
    OPS.iter().find(|op| op.name == name)
}

#[cfg(test)]
mod tests;
