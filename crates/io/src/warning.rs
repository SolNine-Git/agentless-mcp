//! The warnings that a project config load gives, as typed values.

use std::fmt;

use json::JsonError;
use prompts::messages::{
    CONFIG_BOUNDARY_GLOBS_DROPPED, CONFIG_BOUNDARY_GLOBS_NOT_LIST, CONFIG_BOUNDARY_GLOBS_TOO_MANY,
    CONFIG_EROSION_GATE_MODE, CONFIG_EROSION_GATE_NOT_OBJECT, CONFIG_EROSION_GATE_UNKNOWN,
    CONFIG_STOPLIST_DROPPED, CONFIG_UNKNOWN_KEYS_SUPPRESSED,
};
use text::CodePointString;

use crate::config::{
    CONFIG_FILENAME, GateMode, GatedCheck, Granularity, KNOWN_KEYS, MAX_BOUNDARY_GLOB_CHARS,
    MAX_BOUNDARY_GLOBS, MAX_CONFIG_BYTES,
};
use crate::keys::{MAX_STOPLIST_ENTRIES, MAX_STOPLIST_ENTRY_CHARS, MAX_TEST_CMD_CHARS};

/// One problem that a config load found. It displays as its warning text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigWarning {
    /// The file was not used.
    File(FileRefusal),
    /// An integer, choice or boolean value was ignored.
    Value(ValueWarning),
    /// The `stoplist` key or some of its entries were ignored.
    Stoplist(StoplistWarning),
    /// The `test_cmd` key was ignored.
    TestCmd(TestCmdWarning),
    /// A key that the config does not define was ignored.
    Unknown(UnknownKeyWarning),
    /// The `erosion_gate` key or some of its entries were ignored.
    ErosionGate(GateWarning),
    /// The `boundary_globs` key or some of its entries were ignored.
    BoundaryGlobs(GlobsWarning),
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(refusal) => refusal.fmt(f),
            Self::Value(value) => value.fmt(f),
            Self::Stoplist(stoplist) => stoplist.fmt(f),
            Self::TestCmd(test_cmd) => test_cmd.fmt(f),
            Self::Unknown(unknown) => unknown.fmt(f),
            Self::ErosionGate(gate) => gate.fmt(f),
            Self::BoundaryGlobs(globs) => globs.fmt(f),
        }
    }
}

/// Why the config file was not used. It displays as the warning text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRefusal {
    /// Something other than a regular file is at the config path.
    NotRegular,
    /// The config path resolves outside the repository root.
    Outside,
    /// The stat of the opened file failed, with the OS message.
    Unstatable(String),
    /// The file holds more bytes than the config cap.
    TooLarge {
        /// The size from the stat.
        size: u64,
    },
    /// The open or the read failed, with the OS message.
    Unreadable(String),
    /// The file grew past the config cap during the read.
    Grew,
    /// The file is not UTF-8 text.
    NotUtf8,
    /// The text is not JSON.
    NotJson(JsonError),
    /// The JSON value is not an object.
    NotObject,
}

impl fmt::Display for FileRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = CONFIG_FILENAME;
        match self {
            Self::NotRegular => write!(f, "{name} is not a readable regular file; ignored"),
            Self::Outside => write!(f, "{name} resolves outside the repository root; ignored"),
            Self::Unstatable(reason) => write!(f, "{name} could not be stat'd: {reason}"),
            Self::TooLarge { size } => write!(
                f,
                "{name} is {size} bytes, over the {MAX_CONFIG_BYTES}-byte cap; ignored"
            ),
            Self::Unreadable(reason) => write!(f, "{name} could not be read: {reason}"),
            Self::Grew => write!(
                f,
                "{name} grew past the {MAX_CONFIG_BYTES}-byte cap while it was being read; ignored"
            ),
            Self::NotUtf8 => write!(f, "{name} is not UTF-8 text; ignored"),
            Self::NotJson(error) => write!(
                f,
                "{name} is not valid JSON ({} at line {})",
                error.kind, error.line
            ),
            Self::NotObject => write!(f, "{name} must hold a JSON object; ignored"),
        }
    }
}

/// An integer, choice or boolean value that was ignored. It displays as the warning text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueWarning {
    /// The value is not an integer.
    NotInteger { key: &'static str },
    /// The integer is outside its bounds.
    OutOfRange {
        key: &'static str,
        /// The least value allowed.
        low: u32,
        /// The greatest value allowed.
        high: u32,
    },
    /// The value is not a [`Granularity`] name.
    NotChoice { key: &'static str },
    /// The value is not `true` or `false`.
    NotBoolean { key: &'static str },
}

impl fmt::Display for ValueWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = CONFIG_FILENAME;
        match self {
            Self::NotInteger { key } => write!(f, "{key} in {name} must be an integer; ignored"),
            Self::OutOfRange { key, low, high } => {
                write!(
                    f,
                    "{key} in {name} must be between {low} and {high}; ignored"
                )
            }
            Self::NotChoice { key } => {
                let choices = Granularity::ALL.map(Granularity::name).join(", ");
                write!(f, "{key} in {name} must be one of {choices}; ignored")
            }
            Self::NotBoolean { key } => write!(f, "{key} in {name} must be true or false; ignored"),
        }
    }
}

/// A `stoplist` problem. It displays as the warning text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoplistWarning {
    /// The value is not a list; the key is ignored.
    NotList,
    /// The list holds more entries than the stoplist cap. The rest are ignored.
    OverCap {
        /// The entries in the list.
        count: usize,
    },
    /// Entries that are not bare names were dropped.
    Dropped {
        /// The entries dropped.
        count: usize,
    },
}

impl fmt::Display for StoplistWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = CONFIG_FILENAME;
        match self {
            Self::NotList => write!(f, "stoplist in {name} must be a list of names; ignored"),
            Self::OverCap { count } => write!(
                f,
                "stoplist in {name} holds {count} entries, over the {MAX_STOPLIST_ENTRIES} cap; \
                 only the first {MAX_STOPLIST_ENTRIES} are used"
            ),
            Self::Dropped { count } => f.write_str(&CONFIG_STOPLIST_DROPPED.render((
                name,
                *count,
                MAX_STOPLIST_ENTRY_CHARS,
            ))),
        }
    }
}

/// Why `test_cmd` was ignored. It displays as the warning text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestCmdWarning {
    /// The value is not a string, or it holds only whitespace.
    Empty,
    /// The string is longer than the test command cap.
    TooLong,
    /// The string holds a line break.
    Multiline,
}

impl fmt::Display for TestCmdWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = CONFIG_FILENAME;
        match self {
            Self::Empty => write!(f, "test_cmd in {name} must be a non-empty string; ignored"),
            Self::TooLong => {
                write!(
                    f,
                    "test_cmd in {name} is longer than {MAX_TEST_CMD_CHARS} characters; ignored"
                )
            }
            Self::Multiline => write!(f, "test_cmd in {name} must be a single line; ignored"),
        }
    }
}

/// An ignored key that the config does not define. It displays as the warning text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnknownKeyWarning {
    /// One unknown key.
    Key(CodePointString),
    /// The unknown keys past the warning cap, which get no warning of their own.
    Suppressed {
        /// The keys without a warning.
        count: usize,
    },
}

impl fmt::Display for UnknownKeyWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = CONFIG_FILENAME;
        match self {
            Self::Key(key) => write!(
                f,
                "unknown key {} in {name}: ignored (known keys: {})",
                key.quoted(),
                KNOWN_KEYS.join(", ")
            ),
            Self::Suppressed { count } => {
                f.write_str(&CONFIG_UNKNOWN_KEYS_SUPPRESSED.render((name, *count)))
            }
        }
    }
}

/// An `erosion_gate` problem. It displays as the prompts text of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateWarning {
    /// `config_erosion_gate_not_object`: the value is not an object.
    NotObject,
    /// `config_erosion_gate_mode`: the check's mode is not a known mode.
    Mode(GatedCheck),
    /// `config_erosion_gate_unknown`: entries that name no gated check.
    Unknown { count: usize },
}

impl fmt::Display for GateWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let modes = GateMode::ALL.map(GateMode::name).join(", ");
        let text = match *self {
            Self::NotObject => CONFIG_EROSION_GATE_NOT_OBJECT.render((CONFIG_FILENAME, &modes)),
            Self::Mode(check) => {
                CONFIG_EROSION_GATE_MODE.render((CONFIG_FILENAME, check.name(), &modes))
            }
            Self::Unknown { count } => {
                let checks = GatedCheck::ALL.map(GatedCheck::name).join(", ");
                CONFIG_EROSION_GATE_UNKNOWN.render((CONFIG_FILENAME, &checks, count))
            }
        };
        f.write_str(&text)
    }
}

/// A `boundary_globs` problem. It displays as the prompts text of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobsWarning {
    /// `config_boundary_globs_not_list`: the value is not a list.
    NotList,
    /// `config_boundary_globs_too_many`: the list holds more globs than [`MAX_BOUNDARY_GLOBS`].
    TooMany {
        /// The globs in the list.
        count: usize,
    },
    /// `config_boundary_globs_dropped`: entries that are not repo-relative globs.
    Dropped {
        /// The entries dropped.
        count: usize,
    },
}

impl fmt::Display for GlobsWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (limit, chars) = (MAX_BOUNDARY_GLOBS, MAX_BOUNDARY_GLOB_CHARS);
        let text = match *self {
            Self::NotList => CONFIG_BOUNDARY_GLOBS_NOT_LIST.render((CONFIG_FILENAME, limit, chars)),
            Self::TooMany { count } => {
                CONFIG_BOUNDARY_GLOBS_TOO_MANY.render((CONFIG_FILENAME, count, limit))
            }
            Self::Dropped { count } => {
                CONFIG_BOUNDARY_GLOBS_DROPPED.render((CONFIG_FILENAME, count, chars))
            }
        };
        f.write_str(&text)
    }
}
