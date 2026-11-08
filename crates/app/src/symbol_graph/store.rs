//! The scans that the parts of one kept state borrow: the first scan of the state, and one more
//! for each update. A scan is never changed or dropped before the parts that read it.

use std::ptr;
use std::sync::{Arc, OnceLock};

use graph::FileFacts;

// One scan: the facts that a call read, the list of them that every part borrows, and the scan
// after it. A lock sets each once, so a scan never changes after its parts read it.
pub(super) struct Scan<'s> {
    files: Arc<Vec<Arc<FileFacts>>>,
    listed: OnceLock<Vec<&'s FileFacts>>,
    next: OnceLock<Box<Scan<'s>>>,
}

impl<'s> Scan<'s> {
    pub(super) fn new(files: &Arc<Vec<Arc<FileFacts>>>) -> Scan<'s> {
        Scan {
            files: Arc::clone(files),
            listed: OnceLock::new(),
            next: OnceLock::new(),
        }
    }

    // The scan of `files` after this one. Its list keeps this scan's facts of each file whose
    // facts did not change, so that an update finds the changed files by their address.
    pub(super) fn then(&'s self, files: &Arc<Vec<Arc<FileFacts>>>) -> &'s Scan<'s> {
        let next = self.next.get_or_init(|| Box::new(Scan::new(files)));
        (next.listed).get_or_init(|| carried(self.listed(), &next.files));
        next
    }

    pub(super) fn files(&self) -> &Arc<Vec<Arc<FileFacts>>> {
        &self.files
    }

    // The list borrows the scan for as long as the parts that read it.
    pub(super) fn listed(&'s self) -> &'s [&'s FileFacts] {
        (self.listed).get_or_init(|| self.files.iter().map(|file| &**file).collect())
    }
}

fn carried<'s>(before: &[&'s FileFacts], files: &'s [Arc<FileFacts>]) -> Vec<&'s FileFacts> {
    let same_paths = before.len() == files.len();
    let kept = |(at, file): (usize, &'s Arc<FileFacts>)| match before.get(at) {
        Some(held) if same_paths && (ptr::eq(*held, Arc::as_ptr(file)) || **held == **file) => {
            *held
        }
        _ => &**file,
    };
    files.iter().enumerate().map(kept).collect()
}
