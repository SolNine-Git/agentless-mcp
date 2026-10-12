//! The repository walk: the files git lists under a root it owns, or the bounded walk of the tree.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::{Git, GitOutcome, Limits, Unfinished};

// `--others` lists the whole untracked tree, so this listing is as large as a repository the tool
// does not own. The file bound accepts far less.
const LISTING: Limits = Limits {
    seconds: 30,
    shown: "30",
    cap: 16_000_000,
};

// `check-ignore -q` prints nothing, so this cap bounds only a git that answers with something else.
const IGNORE: Limits = Limits {
    seconds: 30,
    shown: "30",
    cap: 65_536,
};

const BOUNDS: Bounds = Bounds {
    depth: io::MAX_WALK_DEPTH,
    files: io::MAX_WALK_FILES,
    bytes: io::MAX_WALK_BYTES,
};

/// One file under the walked root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoFile {
    /// The path below the root, with "/" between its parts.
    pub path: String,
    /// The size in bytes.
    pub size: u64,
}

/// The files a walk lists, sorted by path, and what the walk passed over.
#[derive(Debug)]
pub struct Walk {
    /// Each file, sorted by path.
    pub files: Vec<RepoFile>,
    /// Each warning, in the order the walk raised it. It displays as a log line.
    pub warnings: Vec<WalkWarning>,
}

/// Something the walk passed over or could not check. It displays as a log line.
#[derive(Debug)]
pub enum WalkWarning {
    /// Git did not say whether the enclosing repository ignores the root, so git lists it.
    IgnoreUnchecked {
        /// The top level of the enclosing repository.
        top: PathBuf,
        /// The walked root.
        root: PathBuf,
        /// Why git gave no answer.
        reason: Unfinished,
    },
    /// Git listed names that are not UTF-8, and the walk omits them.
    NotUtf8 {
        /// The number of names.
        count: usize,
        /// The walked root.
        root: PathBuf,
    },
    /// The bounded walk found names that are not UTF-8, and the walk omits them.
    NotUtf8Walked {
        /// The number of names.
        count: usize,
        /// The walked root.
        root: PathBuf,
    },
    /// A directory or a file that the walk could not read or measure.
    Skipped(io::WalkSkip),
}

/// Why the walk refused the root. It displays as the refusal text.
#[derive(Debug)]
pub enum WalkError {
    /// The root is not a directory, or the files pass the file or byte bound.
    Refused(io::WalkRefusal),
    /// A listed path lies deeper than the depth bound.
    TooDeep {
        /// The path's depth below the root.
        depth: usize,
        /// The bound.
        limit: usize,
        /// The walked root.
        root: PathBuf,
    },
    /// Git listed more bytes of names than the listing cap.
    ListingCapped {
        /// The walked root.
        root: PathBuf,
    },
    /// Git did not list the root.
    Unlisted {
        /// The walked root.
        root: PathBuf,
        /// Why git gave no listing.
        note: String,
    },
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    depth: usize,
    files: usize,
    bytes: u64,
}

// The names below the root, and the warnings raised while finding them.
type Found = (Vec<String>, Vec<WalkWarning>);

impl Git {
    /// Lists each file under `root` that git does not ignore, within the walk bounds.
    ///
    /// # Errors
    ///
    /// Returns [`WalkError`] when the root is not a directory, git cannot list it, or the files
    /// pass a bound.
    pub fn walk(&self, root: &Path) -> Result<Walk, WalkError> {
        self.walk_within(root, BOUNDS)
    }

    fn walk_within(&self, root: &Path, bounds: Bounds) -> Result<Walk, WalkError> {
        let root = resolved_directory(root)?;
        let (names, mut warnings) = self.names(&root, bounds.files)?;
        let inside = |candidate: &Path| io::file_stays_inside(candidate, &root);
        let (files, skipped) = admit(&root, names, bounds, inside, io::file_size)?;
        warnings.extend(skipped);
        Ok(Walk { files, warnings })
    }

    fn names(&self, root: &Path, max_files: usize) -> Result<Found, WalkError> {
        let (listed, unchecked) = self.listing_speaks_for(root);
        let (names, warnings) = if listed {
            self.listed(root, max_files)?
        } else {
            walked(root)?
        };
        Ok((names, unchecked.into_iter().chain(warnings).collect()))
    }

    // Git's listing describes the root only when the answering repository owns it. A root that the
    // enclosing work tree ignores lists empty, so the bounded walk serves it.
    fn listing_speaks_for(&self, root: &Path) -> (bool, Option<WalkWarning>) {
        match self.root(root) {
            None => (false, None),
            Some(top) if top == root => (true, None),
            Some(top) => {
                let (ignored, warning) = self.ignores(&top, root);
                (!ignored, warning)
            }
        }
    }

    fn ignores(&self, top: &Path, root: &Path) -> (bool, Option<WalkWarning>) {
        let rest = [OsStr::new("-q"), OsStr::new("--"), root.as_os_str()];
        ignore_verdict(top, root, self.run(top, "check-ignore", &rest, IGNORE))
    }

    fn listed(&self, root: &Path, max_files: usize) -> Result<Found, WalkError> {
        let rest = ["--cached", "--others", "--exclude-standard", "-z"].map(OsStr::new);
        listed_names(root, self.run(root, "ls-files", &rest, LISTING), max_files)
    }
}

impl fmt::Display for WalkWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IgnoreUnchecked { top, root, reason } => write!(
                f,
                "git check-ignore could not be run in {} for {root} ({reason}). The listing for \
                 {root} comes from git, as it did before.",
                top.display(),
                root = root.display()
            ),
            Self::NotUtf8 { count, root } => write!(
                f,
                "git listed {count} path(s) under {} whose names are not valid UTF-8. The tree \
                 omits them.",
                root.display()
            ),
            Self::NotUtf8Walked { count, root } => write!(
                f,
                "the walk found {count} path(s) under {} whose names are not valid UTF-8. The \
                 tree omits them.",
                root.display()
            ),
            Self::Skipped(skip) => skip.fmt(f),
        }
    }
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::TooDeep { depth, limit, root } => write!(
                f,
                "walk refused: path depth {depth} exceeds the limit of {limit} under {}; point the \
                 call at a subdirectory instead",
                root.display()
            ),
            Self::ListingCapped { root } => write!(
                f,
                "walk refused: git listed more than {} bytes of names under {}; point the call at \
                 a subdirectory instead",
                LISTING.cap,
                root.display()
            ),
            Self::Unlisted { root, note } => write!(
                f,
                "{} cannot be listed the way its .gitignore asks: {note}",
                root.display()
            ),
        }
    }
}

impl From<WalkError> for kernel::Error {
    fn from(error: WalkError) -> Self {
        match error {
            WalkError::Refused(refusal) => refusal.into(),
            WalkError::Unlisted { .. } => Self::RepoResolution(error.to_string()),
            WalkError::TooDeep { .. } | WalkError::ListingCapped { .. } => {
                Self::WalkBoundExceeded(error.to_string())
            }
        }
    }
}

fn resolved_directory(root: &Path) -> Result<PathBuf, WalkError> {
    let refused = |root: PathBuf| WalkError::Refused(io::WalkRefusal::NotDirectory { root });
    let resolved = io::resolve(root).map_err(|_| refused(root.to_path_buf()))?;
    if resolved.is_dir() {
        Ok(resolved)
    } else {
        Err(refused(resolved))
    }
}

// Exit 0 says ignored and exit 1 says not. Any other answer keeps the git listing, and only a
// missing exit status is worth a warning.
fn ignore_verdict(top: &Path, root: &Path, outcome: GitOutcome) -> (bool, Option<WalkWarning>) {
    match outcome {
        GitOutcome::Printed(_) => (true, None),
        GitOutcome::Capped(_) | GitOutcome::Failed(_) => (false, None),
        GitOutcome::Unfinished(reason) => {
            let warning = WalkWarning::IgnoreUnchecked {
                top: top.to_path_buf(),
                root: root.to_path_buf(),
                reason,
            };
            (false, Some(warning))
        }
    }
}

fn listed_names(root: &Path, outcome: GitOutcome, max_files: usize) -> Result<Found, WalkError> {
    let unlisted = |note: String| WalkError::Unlisted {
        root: root.to_path_buf(),
        note,
    };
    let stdout = match outcome {
        GitOutcome::Printed(stdout) => stdout,
        GitOutcome::Capped(_) => {
            let root = root.to_path_buf();
            return Err(WalkError::ListingCapped { root });
        }
        GitOutcome::Failed(failure) => return Err(unlisted(failure.to_string())),
        GitOutcome::Unfinished(reason) => return Err(unlisted(reason.to_string())),
    };
    let (names, count) = decoded_names(&stdout, root, max_files)?;
    let root = root.to_path_buf();
    let warning = (count > 0).then_some(WalkWarning::NotUtf8 { count, root });
    Ok((names, warning.into_iter().collect()))
}

// Each name is decoded strictly and on its own, so a name that is not UTF-8 is counted rather than
// listed as a file that does not exist. The bound applies while the listing is read.
fn decoded_names(
    stdout: &[u8],
    root: &Path,
    max_files: usize,
) -> Result<(Vec<String>, usize), WalkError> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    let mut undecodable = 0;
    for record in stdout
        .split(|&byte| byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Ok(name) = std::str::from_utf8(record) else {
            undecodable += 1;
            continue;
        };
        // An unmerged index lists one path once per conflict stage.
        if !seen.insert(name) {
            continue;
        }
        names.push(name.to_owned());
        if names.len() > max_files {
            return Err(too_many_files(max_files, root));
        }
    }
    Ok((names, undecodable))
}

fn walked(root: &Path) -> Result<Found, WalkError> {
    let walk = io::walk(root).map_err(WalkError::Refused)?;
    let mut warnings: Vec<WalkWarning> =
        walk.skipped.into_iter().map(WalkWarning::Skipped).collect();
    let mut names = Vec::new();
    for file in &walk.files {
        // A path outside the root keeps its absolute form, which the containment check refuses.
        let relative = file.strip_prefix(root).unwrap_or(file);
        names.extend(relative.to_str().map(str::to_owned));
    }
    let count = walk.files.len() - names.len();
    let root = root.to_path_buf();
    warnings.extend((count > 0).then_some(WalkWarning::NotUtf8Walked { count, root }));
    Ok((names, warnings))
}

// Keeps each name whose file stays inside the root and can be measured, in path order, and refuses
// the walk at the first bound it passes.
fn admit(
    root: &Path,
    mut names: Vec<String>,
    bounds: Bounds,
    inside: impl Fn(&Path) -> bool,
    size: impl Fn(&Path) -> std::io::Result<u64>,
) -> Result<(Vec<RepoFile>, Vec<WalkWarning>), WalkError> {
    names.sort_unstable();
    names.dedup();
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut total: u64 = 0;
    for name in names {
        let candidate = root.join(&name);
        // Git lists an index entry for a file deleted in the tree, and a tracked symlink wherever
        // it points. The containment rule of the bounded walk drops both.
        if !inside(&candidate) {
            continue;
        }
        let depth = Path::new(&name).components().count().saturating_sub(1);
        if depth > bounds.depth {
            let (limit, root) = (bounds.depth, root.to_path_buf());
            return Err(WalkError::TooDeep { depth, limit, root });
        }
        let size = match size(&candidate) {
            Ok(size) => size,
            Err(error) => {
                let path = candidate;
                warnings.push(WalkWarning::Skipped(io::WalkSkip::Unstatable {
                    path,
                    error,
                }));
                continue;
            }
        };
        total = total.saturating_add(size);
        if files.len() >= bounds.files {
            return Err(too_many_files(bounds.files, root));
        }
        if total > bounds.bytes {
            let (limit, root) = (bounds.bytes, root.to_path_buf());
            return Err(WalkError::Refused(io::WalkRefusal::TooManyBytes {
                limit,
                root,
            }));
        }
        files.push(RepoFile { path: name, size });
    }
    Ok((files, warnings))
}

fn too_many_files(limit: usize, root: &Path) -> WalkError {
    let root = root.to_path_buf();
    WalkError::Refused(io::WalkRefusal::TooManyFiles { limit, root })
}

#[cfg(test)]
mod tests;
