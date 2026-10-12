//! The failures of a database read, and the reasons a database is refused.

use std::fmt;

use prompts::messages::{
    CACHE_DISCARDED_NO_INDEX, CACHE_DISCARDED_OLD_SCHEMA, CACHE_DISCARDED_OTHER_REPO,
};
use pycompat::PyStr;

use crate::SCHEMA_VERSION;

/// The class of a read failure. It decides whether a refused database is discarded, and it names
/// the failure in a row warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlClass {
    /// The environment failed: an I/O error, a lock wait, or a file that does not open.
    Operational,
    /// The file is not a usable database: it is corrupt, or it is not a database.
    Database,
    /// A constraint or a type check failed.
    Integrity,
    /// The database engine reported an internal error.
    Internal,
    /// A value is too large.
    Data,
    /// The database interface was misused.
    Interface,
    /// A stored value has a content that its reader refuses.
    Value,
    /// A stored value has a type that its reader refuses.
    Type,
    /// A stored JSON text does not parse.
    JsonDecode,
}

impl SqlClass {
    // An extended result code keeps its primary code in the low byte.
    pub(crate) fn of_code(extended: i32) -> Self {
        match extended & 0xff {
            1 | 3..=10 | 13..=17 => Self::Operational,
            2 | 12 => Self::Internal,
            18 => Self::Data,
            19 | 20 => Self::Integrity,
            21 | 25 => Self::Interface,
            _ => Self::Database,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Operational => "OperationalError",
            Self::Database => "DatabaseError",
            Self::Integrity => "IntegrityError",
            Self::Internal => "InternalError",
            Self::Data => "DataError",
            Self::Interface => "InterfaceError",
            Self::Value => "ValueError",
            Self::Type => "TypeError",
            Self::JsonDecode => "JSONDecodeError",
        }
    }

    // Only a failure of the file itself discards it: the environment and this reader's own
    // conversions say nothing about what the index holds.
    fn discards(self) -> bool {
        matches!(
            self,
            Self::Database | Self::Integrity | Self::Internal | Self::Data
        )
    }
}

/// A read failure: its class and its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlError {
    /// The class of the failure.
    pub class: SqlClass,
    /// The message, such as `database disk image is malformed`.
    pub message: String,
}

impl SqlError {
    pub(crate) fn new(class: SqlClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    /// Returns the class name with the quoted message, such as
    /// `DatabaseError('database disk image is malformed')`.
    #[must_use]
    pub fn repr(&self) -> String {
        let message = PyStr::from(self.message.as_str()).repr();
        format!("{}({message})", self.class.name())
    }
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SqlError {}

impl SqlError {
    // A failure that SQLite reports keeps its message; any other binding error is a misuse.
    pub(crate) fn from_sqlite(error: rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::SqliteFailure(failure, message) => {
                let message = message.unwrap_or_else(|| failure.to_string());
                Self::new(SqlClass::of_code(failure.extended_code), message)
            }
            other => Self::new(SqlClass::Interface, other.to_string()),
        }
    }
}

/// Why a database is not used. It displays as the receipt note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The database could not be read. It stays in place.
    Unreadable(SqlError),
    /// The file is not a usable database.
    Corrupt(SqlError),
    /// The database holds no meta table or no meta row.
    NoIndex,
    /// The database has another schema version.
    Schema {
        /// The schema version that the database records.
        found: i64,
    },
    /// The database was built for another repository.
    OtherRepository {
        /// The repository root that the database records.
        repo_root: String,
    },
}

impl Refusal {
    /// Returns true when the caller discards the refused database.
    #[must_use]
    pub fn discards(&self) -> bool {
        !matches!(self, Self::Unreadable(_))
    }
}

impl From<SqlError> for Refusal {
    fn from(error: SqlError) -> Self {
        if error.class.discards() {
            Self::Corrupt(error)
        } else {
            Self::Unreadable(error)
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(error) => write!(f, "cache unreadable: {error}"),
            Self::Corrupt(error) => write!(f, "cache discarded: {error}"),
            Self::NoIndex => f.write_str(CACHE_DISCARDED_NO_INDEX),
            Self::Schema { found } => {
                let values = (found.to_string(), usize::from(SCHEMA_VERSION));
                f.write_str(&CACHE_DISCARDED_OLD_SCHEMA.render((&values.0, values.1)))
            }
            Self::OtherRepository { repo_root } => {
                f.write_str(&CACHE_DISCARDED_OTHER_REPO.render(repo_root))
            }
        }
    }
}
