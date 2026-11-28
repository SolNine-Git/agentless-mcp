//! The scan's session state: one exclusive-create file for each root that a shell write touched
//! and one for each finding already reported. Layout: `agentless_scan/<sha256(session)>/repos/
//! <sha256(root)>`, which holds the root path, and `.../reported/<sha256(path, NUL, key)>`.

use std::fs;
use std::path::{Path, PathBuf};

use app::EnvConfig;

use crate::state::{self, Claim};
use crate::text::sha256_hex;

const SCAN_DIR: &str = "agentless_scan";
const REPOS: &str = "repos";
const REPORTED: &str = "reported";
// The entries of one folder. Past the cap a folder takes no new entry.
pub(crate) const CAP: usize = 2_000;

pub(crate) fn base(env: &EnvConfig) -> PathBuf {
    state::temporary_dir(env, SCAN_DIR)
}

// The state of one session below `base`.
pub(crate) struct Markers {
    base: PathBuf,
    session: PathBuf,
}

impl Markers {
    pub(crate) fn new(base: &Path, session: &str) -> Self {
        Self {
            session: base.join(sha256_hex(session.as_bytes())),
            base: base.to_path_buf(),
        }
    }

    // Marks the root, so that a Stop scans it. False when the mark cannot be kept.
    pub(crate) fn mark(&self, root: &Path) -> bool {
        let content = root.as_os_str().as_encoded_bytes();
        let name = sha256_hex(content);
        matches!(
            self.claimed(REPOS, &name, content),
            Claim::New | Claim::Held
        )
    }

    // The marked roots in path order, each as the marker holds it.
    pub(crate) fn marked(&self) -> Vec<PathBuf> {
        // Another user can plant markers in a base folder that is not private.
        if !state::private_dir(&self.base) {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(self.session.join(REPOS)) else {
            return Vec::new();
        };
        let mut roots: Vec<PathBuf> = (entries.take(CAP).filter_map(Result::ok))
            .map(|entry| entry.path())
            .filter(|path| state::is_plain_file(path))
            .filter_map(|path| fs::read(path).ok())
            .filter_map(|bytes| String::from_utf8(bytes).ok())
            .map(PathBuf::from)
            .filter(|root| root.is_absolute())
            .collect();
        roots.sort();
        roots
    }

    // True when the finding was not reported before, or when the state cannot say: a duplicate
    // report is better than a lost one.
    pub(crate) fn first_report(&self, path: &str, key: &str) -> bool {
        let name = sha256_hex(format!("{path}\0{key}").as_bytes());
        self.claimed(REPORTED, &name, b"") != Claim::Held
    }

    fn claimed(&self, folder: &str, name: &str, content: &[u8]) -> Claim {
        if !state::private_dir(&self.base) {
            return Claim::Unavailable;
        }
        let dir = self.session.join(folder);
        if io::create_missing_owner_dir(&dir).is_err() {
            return Claim::Unavailable;
        }
        let entry = dir.join(name);
        if !state::is_plain_file(&entry) && full(&dir) {
            return Claim::Unavailable;
        }
        state::claim_with(&entry, content)
    }
}

fn full(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| entries.take(CAP).count() >= CAP)
}

#[cfg(test)]
mod tests;
