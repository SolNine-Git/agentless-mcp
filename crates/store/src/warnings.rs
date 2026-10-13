//! The texts of the stderr warnings that the index writer and the ceiling sweep give.

use std::path::Path;

use crate::SqlError;

// An operation on a file that failed. Its warning ends with the repr of the OS error, as the
// reference build's `%r` shows it.
pub(crate) enum Failed<'a> {
    Restricting(&'a Path),
    Listing(&'a Path),
    Weighing(&'a Path, &'a str),
    Recheck(&'a Path),
    Eviction(&'a Path),
}

impl Failed<'_> {
    pub(crate) fn warning(&self, error: &std::io::Error) -> String {
        let text = match self {
            Self::Restricting(path) => {
                format!("could not restrict {} to owner-only", path.display())
            }
            Self::Listing(root) => format!(
                "cache root {} could not be listed, so nothing was evicted",
                root.display()
            ),
            Self::Weighing(database, suffix) => format!(
                "could not weigh {}{suffix} for the cache ceiling",
                database.display()
            ),
            Self::Recheck(database) => {
                format!(
                    "could not re-check {} before eviction; kept",
                    database.display()
                )
            }
            Self::Eviction(database) => format!("could not evict {}", database.display()),
        };
        format!("{text}: {}", io::os_error_class_text(error))
    }
}

// A migration succeeded and its freed pages stay in the file, because VACUUM failed.
pub(crate) fn unreclaimed(error: &SqlError) -> String {
    format!(
        "could not reclaim free pages after the schema migration ({}); the database is correct \
         but still holds the space the old schema used. A VACUUM needs free disk for a second \
         copy of the file",
        error.repr()
    )
}
