//! A bounded walk of a directory tree: each file that stays inside the root, in sorted order.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirEntry};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use text::CodePointString;

use crate::{contained_file_size, resolve, strerror};

/// The deepest directory, in levels below the root, that [`walk`] enters.
pub const MAX_WALK_DEPTH: usize = 20;
/// The most files that [`walk`] returns.
pub const MAX_WALK_FILES: usize = 20_000;
/// The most bytes, summed over the files, that [`walk`] returns.
pub const MAX_WALK_BYTES: u64 = 200_000_000;

/// The files a walk found, or the refusal that stopped it, and what it passed over first.
#[derive(Debug)]
pub struct Walk {
    /// Each file, as a path below the resolved root, in walk order, or why the walk refused.
    pub files: Result<Vec<PathBuf>, WalkRefusal>,
    /// Each directory or file the walk passed over, in walk order, those before a refusal
    /// included.
    pub skipped: Vec<WalkSkip>,
}

/// A directory or a file that the walk passed over. It displays as a log line.
#[derive(Debug)]
pub enum WalkSkip {
    /// The directory could not be listed.
    Unlisted {
        /// The directory.
        path: PathBuf,
        /// The listing error.
        error: io::Error,
    },
    /// The directory or the file could not be stat'ed.
    Unstatable {
        /// The directory or the file.
        path: PathBuf,
        /// The stat error.
        error: io::Error,
    },
    /// The directory's device and inode were already walked: a cycle or a second mount.
    Revisited {
        /// The directory.
        path: PathBuf,
        device: u64,
        inode: u64,
    },
}

impl fmt::Display for WalkSkip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unlisted { path, error } | Self::Unstatable { path, error } => {
                write!(f, "walk skipped {}: {}", path.display(), strerror(error))
            }
            Self::Revisited {
                path,
                device,
                inode,
            } => write!(
                f,
                "walk pruned {}: device {device} inode {inode} was already visited, so this subtree \
                 is either a cycle or a second mount of one already walked",
                path.display()
            ),
        }
    }
}

/// Why [`walk`] refused a tree. It displays as the refusal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkRefusal {
    /// The resolved root is not a directory.
    NotDirectory {
        /// The resolved root.
        root: PathBuf,
    },
    /// A directory lies deeper than the depth bound.
    TooDeep {
        /// The directory's depth below the root.
        depth: usize,
        limit: usize,
        /// The resolved root.
        root: PathBuf,
    },
    /// The tree holds more files than the file bound.
    TooManyFiles {
        limit: usize,
        /// The resolved root.
        root: PathBuf,
    },
    /// The files hold more bytes than the byte bound.
    TooManyBytes {
        limit: u64,
        /// The resolved root.
        root: PathBuf,
    },
}

impl fmt::Display for WalkRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDirectory { root } => write!(f, "not a directory: {}", root.display()),
            Self::TooDeep { depth, limit, root } => write!(
                f,
                "walk refused: directory depth {depth} exceeds the limit of {limit} under {}; \
                 point the call at a subdirectory instead",
                root.display()
            ),
            Self::TooManyFiles { limit, root } => write!(
                f,
                "walk refused: more than {limit} files under {}; point the call at a subdirectory \
                 or raise the file bound",
                root.display()
            ),
            Self::TooManyBytes { limit, root } => write!(
                f,
                "walk refused: more than {limit} bytes under {}; point the call at a subdirectory \
                 or raise the byte bound",
                root.display()
            ),
        }
    }
}

impl std::error::Error for WalkRefusal {}

impl From<WalkRefusal> for kernel::Error {
    fn from(refusal: WalkRefusal) -> Self {
        match refusal {
            WalkRefusal::NotDirectory { .. } => Self::RepoResolution(refusal.to_string()),
            _ => Self::WalkBoundExceeded(refusal.to_string()),
        }
    }
}

/// Lists each file under `root`, depth first and sorted by name within each directory. It enters
/// no symlinked directory and each directory once, and it lists a symlinked file only when its
/// target stays inside the root. The files are a [`WalkRefusal`] when the root is not a directory
/// or the tree passes a bound.
#[must_use]
pub fn walk(root: &Path) -> Walk {
    let bounds = Bounds {
        depth: MAX_WALK_DEPTH,
        files: MAX_WALK_FILES,
        bytes: MAX_WALK_BYTES,
    };
    walk_within(root, bounds)
}

#[derive(Clone, Copy)]
struct Bounds {
    depth: usize,
    files: usize,
    bytes: u64,
}

fn walk_within(root: &Path, bounds: Bounds) -> Walk {
    let root = match walkable(root) {
        Ok(root) => root,
        Err(refusal) => {
            return Walk {
                files: Err(refusal),
                skipped: Vec::new(),
            };
        }
    };
    let mut walker = Walker {
        bounds,
        seen: HashSet::new(),
        bytes: 0,
        files: Vec::new(),
        skipped: Vec::new(),
        root,
    };
    let finished = walker.run();
    Walk {
        files: finished.map(|()| walker.files),
        skipped: walker.skipped,
    }
}

// The resolved root, when it names a directory.
fn walkable(root: &Path) -> Result<PathBuf, WalkRefusal> {
    let resolved = resolve(root).map_err(|_| WalkRefusal::NotDirectory {
        root: root.to_path_buf(),
    })?;
    if !resolved.is_dir() {
        return Err(WalkRefusal::NotDirectory { root: resolved });
    }
    Ok(resolved)
}

struct Walker {
    root: PathBuf,
    bounds: Bounds,
    seen: HashSet<(u64, u64)>,
    bytes: u64,
    files: Vec<PathBuf>,
    skipped: Vec<WalkSkip>,
}

#[derive(Default)]
struct Listing {
    directories: Vec<OsString>,
    others: Vec<OsString>,
}

impl Walker {
    // Walks the tree from the root, and stops at the first bound that the tree passes.
    fn run(&mut self) -> Result<(), WalkRefusal> {
        let mut pending = vec![self.root.clone()];
        while let Some(directory) = pending.pop() {
            pending.extend(self.visit(&directory)?.into_iter().rev());
        }
        Ok(())
    }

    // Takes the files of `directory` and returns its subdirectories in walk order.
    fn visit(&mut self, directory: &Path) -> Result<Vec<PathBuf>, WalkRefusal> {
        let listing = match list(directory) {
            Ok(listing) => listing,
            Err(error) => {
                let path = directory.to_path_buf();
                self.skipped.push(WalkSkip::Unlisted { path, error });
                return Ok(Vec::new());
            }
        };
        self.check_depth(directory)?;
        if !self.claim(directory) {
            return Ok(Vec::new());
        }
        for name in sorted(listing.others) {
            self.take_file(directory.join(name))?;
        }
        let subdirectories = sorted(listing.directories)
            .into_iter()
            .map(|name| directory.join(name));
        Ok(subdirectories.filter(|path| !path.is_symlink()).collect())
    }

    fn check_depth(&self, directory: &Path) -> Result<(), WalkRefusal> {
        let depth = directory
            .strip_prefix(&self.root)
            .map_or(0, |relative| relative.components().count());
        if depth <= self.bounds.depth {
            return Ok(());
        }
        Err(WalkRefusal::TooDeep {
            depth,
            limit: self.bounds.depth,
            root: self.root.clone(),
        })
    }

    // Records the directory by device and inode, and returns false when it was walked before.
    fn claim(&mut self, directory: &Path) -> bool {
        let path = directory.to_path_buf();
        let metadata = match fs::metadata(directory) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.skipped.push(WalkSkip::Unstatable { path, error });
                return false;
            }
        };
        let (device, inode) = (metadata.dev(), metadata.ino());
        let fresh = self.seen.insert((device, inode));
        if !fresh {
            self.skipped.push(WalkSkip::Revisited {
                path,
                device,
                inode,
            });
        }
        fresh
    }

    fn take_file(&mut self, candidate: PathBuf) -> Result<(), WalkRefusal> {
        let size = match contained_file_size(&candidate, &self.root) {
            None => return Ok(()),
            Some(Ok(size)) => size,
            Some(Err(error)) => {
                let path = candidate;
                self.skipped.push(WalkSkip::Unstatable { path, error });
                return Ok(());
            }
        };
        let root = self.root.clone();
        if self.files.len() >= self.bounds.files {
            let limit = self.bounds.files;
            return Err(WalkRefusal::TooManyFiles { limit, root });
        }
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > self.bounds.bytes {
            let limit = self.bounds.bytes;
            return Err(WalkRefusal::TooManyBytes { limit, root });
        }
        self.files.push(candidate);
        Ok(())
    }
}

// An entry counts as a directory when it names one, through a symlink too.
fn list(directory: &Path) -> io::Result<Listing> {
    let mut listing = Listing::default();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if names_directory(&entry) {
            listing.directories.push(entry.file_name());
        } else {
            listing.others.push(entry.file_name());
        }
    }
    Ok(listing)
}

fn names_directory(entry: &DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_symlink() => entry.path().is_dir(),
        Ok(kind) => kind.is_dir(),
        Err(_) => false,
    }
}

// Names sort by code point, with each byte that is not UTF-8 read as a lone surrogate.
fn sorted(mut names: Vec<OsString>) -> Vec<OsString> {
    names.sort_by_cached_key(|name| CodePointString::from_utf8_escaped(name.as_bytes()));
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Placed, build, fixture};
    use crate::testing::Tree;
    use serde_json::{Value, json};

    const DEFAULT: Bounds = Bounds {
        depth: MAX_WALK_DEPTH,
        files: MAX_WALK_FILES,
        bytes: MAX_WALK_BYTES,
    };

    fn walked(tree: &Tree, root: &Path, bounds: Bounds) -> Value {
        match walk_within(root, bounds).files {
            Ok(files) => {
                let files: Vec<String> = files
                    .iter()
                    .map(|path| tree.placed(&path.to_string_lossy()))
                    .collect();
                json!(["files", files])
            }
            Err(refusal) => json!(["refused", tree.placed(&refusal.to_string())]),
        }
    }

    // A root that holds `closed`, which mode 000 keeps from being listed, and `open/f`. It returns
    // the walk within `bounds`, and whether the mode refused the listing (it does not for root).
    fn walked_past_a_closed_directory(bounds: Bounds) -> (Tree, Walk, bool) {
        let tree = build(&[
            json!(["dir", "root/closed"]),
            json!(["file", "root/closed/f", "x"]),
            json!(["dir", "root/open"]),
            json!(["file", "root/open/f", "y"]),
        ]);
        let binds = tree.modes_bind();
        tree.chmod("root/closed", 0o000);
        let walked = walk_within(&tree.join("root"), bounds);
        tree.chmod("root/closed", 0o700);
        (tree, walked, binds)
    }

    fn unlisted(walked: &Walk) -> &Path {
        let [WalkSkip::Unlisted { path, .. }] = &walked.skipped[..] else {
            panic!("expected one unlisted record, got {:?}", walked.skipped);
        };
        path
    }

    #[test]
    fn bound_cases() {
        let layout: Vec<Value> = fixture("walk", "layout");
        let tree = build(&layout);
        let all: Vec<((usize, usize, u64), Value)> = fixture("walk", "bounds");
        for ((depth, files, bytes), expected) in all {
            let bounds = Bounds {
                depth,
                files,
                bytes,
            };
            let found = walked(&tree, &tree.join("root"), bounds);
            assert_eq!(found, expected, "bounds {depth} {files} {bytes}");
        }
    }

    #[test]
    fn root_cases() {
        let layout: Vec<Value> = fixture("walk", "layout");
        let tree = build(&layout);
        let all: Vec<(String, Value)> = fixture("walk", "roots");
        for (root, expected) in all {
            assert_eq!(
                walked(&tree, &tree.join(&root), DEFAULT),
                expected,
                "root {root}"
            );
        }
    }

    #[test]
    fn a_directory_seen_before_is_pruned_with_a_record() {
        let tree = build(&[json!(["dir", "d"])]);
        let mut walker = Walker {
            root: tree.path().to_path_buf(),
            bounds: Bounds {
                depth: 1,
                files: 1,
                bytes: 1,
            },
            seen: HashSet::new(),
            bytes: 0,
            files: Vec::new(),
            skipped: Vec::new(),
        };
        let directory = tree.join("d");
        assert!(walker.claim(&directory));
        assert!(!walker.claim(&directory));
        let [WalkSkip::Revisited { path, .. }] = &walker.skipped[..] else {
            panic!("expected one revisit record, got {:?}", walker.skipped);
        };
        assert_eq!(path, &directory);
    }

    #[test]
    fn a_directory_that_cannot_be_listed_is_skipped_with_a_record() {
        let (tree, walked, binds) = walked_past_a_closed_directory(DEFAULT);
        let files = walked
            .files
            .as_ref()
            .expect("the tree is within every bound");
        if !binds {
            assert_eq!(
                files,
                &[tree.join("root/closed/f"), tree.join("root/open/f")]
            );
            return;
        }
        assert_eq!(files, &[tree.join("root/open/f")]);
        assert_eq!(unlisted(&walked), tree.join("root/closed"));
    }

    #[test]
    fn a_refused_walk_keeps_the_skips_before_the_refusal() {
        let bounds = Bounds {
            files: 0,
            ..DEFAULT
        };
        let (tree, walked, binds) = walked_past_a_closed_directory(bounds);
        let refusal = walked.files.as_ref().expect_err("no file fits the bound");
        assert!(
            matches!(refusal, WalkRefusal::TooManyFiles { .. }),
            "{refusal:?}"
        );
        if binds {
            assert_eq!(unlisted(&walked), tree.join("root/closed"));
        }
    }

    #[test]
    fn the_default_walk_lists_the_tree() {
        let tree = build(&[json!(["dir", "d"]), json!(["file", "d/f", "x"])]);
        let files = walk(tree.path())
            .files
            .expect("the tree is within every bound");
        assert_eq!(files, [tree.join("d/f")]);
    }

    #[test]
    fn a_refusal_keeps_its_error_kind() {
        let root = PathBuf::from("/r");
        let missing = kernel::Error::from(WalkRefusal::NotDirectory { root: root.clone() });
        assert_eq!(
            missing,
            kernel::Error::RepoResolution("not a directory: /r".to_owned())
        );
        let crowded = kernel::Error::from(WalkRefusal::TooManyFiles { limit: 1, root });
        assert!(matches!(crowded, kernel::Error::WalkBoundExceeded(_)));
    }
}
