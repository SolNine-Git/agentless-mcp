//! The scans that the parts of one kept state borrow: the first scan of the state, and one more
//! for each update. A scan is never changed or dropped before the parts that read it.

use std::sync::{Arc, OnceLock};

use graph::FileFacts;

// One scan: the facts that a call read, and the list of them that every part borrows.
pub(super) struct Scan<'s> {
    files: Arc<Vec<Arc<FileFacts>>>,
    listed: OnceLock<Vec<&'s FileFacts>>,
}

impl<'s> Scan<'s> {
    pub(super) fn new(files: &Arc<Vec<Arc<FileFacts>>>) -> Scan<'s> {
        Scan {
            files: Arc::clone(files),
            listed: OnceLock::new(),
        }
    }

    pub(super) fn files(&self) -> &Arc<Vec<Arc<FileFacts>>> {
        &self.files
    }

    // The list borrows the scan for as long as the parts that read it.
    pub(super) fn listed(&'s self) -> &'s [&'s FileFacts] {
        (self.listed).get_or_init(|| self.files.iter().map(|file| &**file).collect())
    }
}
