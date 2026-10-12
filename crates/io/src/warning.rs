//! The warnings that a project config load gives, as typed values.

use std::fmt;

use pycompat::{JsonError, PyStr};

use crate::config::{CONFIG_FILENAME, GatedCheck, KNOWN_KEYS, MAX_CONFIG_BYTES};
use crate::keys::{MAX_STOPLIST_ENTRIES, MAX_STOPLIST_ENTRY_CHARS, MAX_TEST_CMD_CHARS};

/// One problem that a config load found. Each inner value displays as its warning text, apart
/// from `ErosionGate` and `BoundaryGlobs`, whose texts belong to the prompts text.
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

/// Why the config file was not used. It displays as the warning text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRefusal {
    /// Something other than a regular file is at the config path.
    NotRegular,
    /// The config path resolves outside the repository root.
    Outside,
    /// The stat of the opened file failed, with the OS message.
    Unstatable(String),
    /// The file holds more than 65,536 bytes.
    TooLarge {
        /// The size from the stat.
        size: u64,
    },
    /// The open or the read failed, with the OS message.
    Unreadable(String),
    /// The file grew past 65,536 bytes during the read.
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
    NotInteger {
        /// The key.
        key: &'static str,
    },
    /// The integer is outside its bounds.
    OutOfRange {
        /// The key.
        key: &'static str,
        /// The least value allowed.
        low: u32,
        /// The greatest value allowed.
        high: u32,
    },
    /// The value is not one of the allowed strings.
    NotChoice {
        /// The key.
        key: &'static str,
        /// The allowed strings, joined by ", ".
        choices: &'static str,
    },
    /// The value is not `true` or `false`.
    NotBoolean {
        /// The key.
        key: &'static str,
    },
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
            Self::NotChoice { key, choices } => {
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
    /// The list holds more than 200 entries; the rest are ignored.
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
            Self::Dropped { count } => write!(
                f,
                "stoplist in {name}: {count} entries are not bare names of at most \
                 {MAX_STOPLIST_ENTRY_CHARS} characters and were dropped"
            ),
        }
    }
}

/// Why `test_cmd` was ignored. It displays as the warning text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestCmdWarning {
    /// The value is not a string, or it holds only whitespace.
    Empty,
    /// The string is longer than 512 characters.
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
    Key(PyStr),
    /// The unknown keys after the first eight, which get no warning of their own.
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
                key.repr(),
                KNOWN_KEYS.join(", ")
            ),
            Self::Suppressed { count } => {
                write!(
                    f,
                    "{count} further unknown keys in {name}: warnings suppressed"
                )
            }
        }
    }
}

/// An `erosion_gate` problem. Its text is the prompts text of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateWarning {
    /// `config_erosion_gate_not_object`: the value is not an object.
    NotObject,
    /// `config_erosion_gate_mode`: the check's mode is not a known mode.
    Mode(GatedCheck),
    /// `config_erosion_gate_unknown`: entries that name no gated check.
    Unknown {
        /// The entries.
        count: usize,
    },
}

/// A `boundary_globs` problem. Its text is the prompts text of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobsWarning {
    /// `config_boundary_globs_not_list`: the value is not a list.
    NotList,
    /// `config_boundary_globs_too_many`: the list holds more than 64 globs.
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
