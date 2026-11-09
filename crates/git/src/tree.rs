//! The walk of a commit's tree: the files that a revision holds below a root, read with no
//! checkout.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

use crate::walk::{BOUNDS, Bounds, LISTING, Walk, WalkWarning, admit, listing, refused};
use crate::{Git, GitOutcome};

// The modes of a regular file, executable or not. A link and a gitlink are passed over, as the
// walk of the working tree passes over them.
const FILE_MODES: [&[u8]; 2] = [b"100644", b"100755"];

impl Git {
    /// Lists each regular file of `commit`'s tree below `root`, with its size from the listing,
    /// within the walk bounds, as [`Git::walk`] lists the working tree. The byte bound counts
    /// only the files that `reads` accepts.
    #[must_use]
    pub fn tree_walk(&self, root: &Path, commit: &str, reads: io::ScanReads) -> Walk {
        // From -C below the top level, ls-tree lists that folder only, with paths relative to it.
        let rest = ["-r", "-z", "--long", "--end-of-options", commit].map(OsStr::new);
        let outcome = self.run_offline(root, "ls-tree", &rest, LISTING);
        tree_files(root, outcome, BOUNDS, reads)
    }
}

fn tree_files(root: &Path, outcome: GitOutcome, bounds: Bounds, reads: io::ScanReads) -> Walk {
    let stdout = match listing(root, outcome) {
        Ok(stdout) => stdout,
        Err(error) => return refused(error, Vec::new()),
    };
    let (sizes, undecodable) = file_sizes(&stdout);
    let mut warnings = Vec::new();
    if undecodable > 0 {
        let (count, root) = (undecodable, root.to_path_buf());
        warnings.push(WalkWarning::NotUtf8 { count, root });
    }
    let names = sizes.keys().cloned().collect();
    let probe = |candidate: &Path| {
        let name = candidate.strip_prefix(root).ok()?.to_str()?;
        sizes.get(name).map(|size| Ok(*size))
    };
    let admitted = admit(root, names, bounds, reads, probe);
    warnings.extend(admitted.warnings);
    Walk {
        files: admitted.files,
        warnings,
    }
}

// The size of each regular file that the listing names, and the count of names that are not
// UTF-8.
fn file_sizes(stdout: &[u8]) -> (HashMap<String, u64>, usize) {
    let mut sizes = HashMap::new();
    let mut undecodable = 0;
    let records = stdout.split(|&byte| byte == 0);
    for (size, name) in records.filter_map(regular_file) {
        match std::str::from_utf8(name) {
            Ok(name) => {
                sizes.insert(name.to_owned(), size);
            }
            Err(_) => undecodable += 1,
        }
    }
    (sizes, undecodable)
}

// The size and the name of a record "<mode> <type> <object> <padded size>\t<name>" that lists a
// regular file, or `None` for any other record.
fn regular_file(record: &[u8]) -> Option<(u64, &[u8])> {
    let tab = record.iter().position(|&byte| byte == b'\t')?;
    let (meta, name) = (&record[..tab], &record[tab + 1..]);
    let mut fields = (meta.split(|&byte| byte == b' ')).filter(|field| !field.is_empty());
    let mode = fields.next()?;
    let size = std::str::from_utf8(fields.nth(2)?).ok()?.parse().ok()?;
    FILE_MODES.contains(&mode).then_some((size, name))
}

#[cfg(test)]
mod tests;
