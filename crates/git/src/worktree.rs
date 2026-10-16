//! Scratch worktrees: a detached checkout of a repository at one commit under the cache root, the
//! diff of what was written in it, and its removal.
//!
//! The scratch directory never lies inside the repository it checks out. Every call is a fixed
//! argv with a timeout. `worktree add` runs no hook of the repository, and the diff runs no
//! external diff or textconv driver of it. A failed creation is released too: a killed `worktree
//! add` leaves a locked record that `worktree prune` skips.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use text::CodePointString;

use crate::run::Call;
use crate::{Git, GitOutcome, Limits};

/// The stdout cap of every write-side git call; the patch diff is the largest output.
pub const MAX_GIT_OUTPUT_BYTES: usize = 8_000_000;
/// The directory below the cache root that holds the scratch worktrees.
pub const WORKTREE_DIR: &str = "worktrees";
/// The `-c` arguments that stop `worktree add` from running the repository's hooks.
pub const NO_REPO_CODE: [&str; 2] = ["-c", "core.hooksPath=/dev/null"];

// A checkout of a large repository takes seconds, never minutes.
pub(crate) const CREATE: Limits = Limits {
    seconds: 120,
    shown: "120.0",
    cap: MAX_GIT_OUTPUT_BYTES,
};
// The cleanup holds the worktree lock, so its bound is how long every other holder waits.
pub(crate) const CLEANUP: Limits = Limits {
    seconds: 30,
    shown: "30.0",
    cap: MAX_GIT_OUTPUT_BYTES,
};

const NAME_BYTES: usize = 16;

// `worktree add` and `worktree prune` race: a prune can remove the record of an add that has not
// populated its directory yet. One lock serializes them in this process.
static WORKTREE_LOCK: Mutex<()> = Mutex::new(());

/// A write-side git call that did not succeed. It displays as the error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunGitError {
    /// Git printed more than [`MAX_GIT_OUTPUT_BYTES`].
    Capped {
        /// The git subcommand.
        subcommand: &'static str,
        /// The directory of the call.
        cwd: PathBuf,
    },
    /// Git failed, timed out or did not start.
    Failed {
        /// The note of the bounded runner.
        note: String,
        /// The directory of the call.
        cwd: PathBuf,
    },
}

/// Why [`Git::pinned_commit`] named no commit. It displays as the error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinError {
    /// The receipt names no commit.
    NoCommit {
        /// The root of the call.
        root: PathBuf,
    },
    /// `rev-parse` failed.
    Git(RunGitError),
}

/// A scratch worktree. The drop releases it when [`Worktree::release`] was not called, and its
/// cleanup notes are lost then.
pub struct Worktree {
    git: Git,
    top: PathBuf,
    path: PathBuf,
    inside: PathBuf,
    released: bool,
}

/// One problem of a release. It displays as its log line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupNote {
    /// `worktree remove` failed, so the directory was removed without git.
    RemoveFailed {
        /// The worktree directory.
        path: PathBuf,
        /// The failure of the call.
        error: RunGitError,
    },
    /// `worktree prune` failed.
    PruneFailed {
        /// The top level of the repository.
        root: PathBuf,
        /// The failure of the call.
        error: RunGitError,
    },
    /// The worktree directory is still on disk.
    Leftover {
        /// The worktree directory.
        path: PathBuf,
    },
}

/// A worktree that was not created, and the notes of the release that followed.
#[derive(Debug)]
pub struct WorktreeFailure {
    /// Why no worktree was created.
    pub error: WorktreeError,
    /// The release notes; empty when the refusal came before the scratch directory was made.
    pub cleanup: Vec<CleanupNote>,
}

/// Why [`Git::worktree`] created no worktree. It displays as the error text.
#[derive(Debug)]
pub enum WorktreeError {
    /// No git repository encloses the root.
    NotRepository {
        /// The resolved root.
        resolved: PathBuf,
    },
    /// The scratch directory resolves inside the repository.
    ScratchInside {
        /// The resolved scratch directory.
        scratch: PathBuf,
        /// The top level of the repository.
        top: PathBuf,
    },
    /// The commit has no directory at the root's place: the root is untracked or ignored.
    NotInCommit {
        /// The resolved root.
        resolved: PathBuf,
        /// The commit of the worktree.
        commit: String,
        /// The top level of the repository.
        top: PathBuf,
        /// The root below the top level.
        relative: PathBuf,
    },
    /// The resolved root is not below the top level that git named.
    OutsideTop {
        /// The resolved root.
        resolved: PathBuf,
        /// The top level of the repository.
        top: PathBuf,
    },
    /// `worktree add` failed.
    Add(RunGitError),
    /// A path could not be resolved, the scratch directory could not be made, or the random
    /// source could not be read.
    Os {
        /// The path of the operation.
        path: PathBuf,
        /// The error of the operation.
        error: std::io::Error,
    },
}

// Where a new worktree goes.
struct Place {
    resolved: PathBuf,
    top: PathBuf,
    path: PathBuf,
}

impl Git {
    /// Returns the full commit that `captured`, a receipt's short id, names, through `rev-parse
    /// --verify` in `root`.
    ///
    /// # Errors
    ///
    /// Returns [`PinError::NoCommit`] for no or an empty `captured`, and the git failure.
    pub fn pinned_commit(&self, root: &Path, captured: Option<&str>) -> Result<String, PinError> {
        let Some(captured) = captured.filter(|captured| !captured.is_empty()) else {
            let root = root.to_path_buf();
            return Err(PinError::NoCommit { root });
        };
        let target = OsString::from(format!("{captured}^{{commit}}"));
        let call = Call {
            config: &[],
            subcommand: "rev-parse",
            rest: &[OsStr::new("--verify"), &target],
        };
        let stdout = self.run_git(root, &call, CREATE).map_err(PinError::Git)?;
        Ok(text::strip(&String::from_utf8_lossy(&stdout)).to_owned())
    }

    /// Returns the unified diff of the uncommitted changes in `dir`, with no color, no external
    /// diff and no textconv driver, decoded with U+FFFD for each invalid sequence.
    ///
    /// # Errors
    ///
    /// Returns the git failure.
    pub fn worktree_diff(&self, dir: &Path) -> Result<String, RunGitError> {
        let call = Call {
            config: &[],
            subcommand: "diff",
            rest: &["--no-color", "--no-ext-diff", "--no-textconv"].map(OsStr::new),
        };
        let stdout = self.run_git(dir, &call, CREATE)?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// Adds a detached worktree of the repository that encloses `root`, at `commit`, under
    /// `<cache_root>/worktrees`, and returns it with the directory that matches `root` in it.
    ///
    /// # Errors
    ///
    /// Returns the [`WorktreeFailure`]; a failure after the add carries the release notes.
    pub fn worktree(
        &self,
        root: &Path,
        commit: &str,
        cache_root: &Path,
    ) -> Result<Worktree, WorktreeFailure> {
        let place = self
            .place(root, cache_root)
            .map_err(|error| WorktreeFailure {
                error,
                cleanup: Vec::new(),
            })?;
        let mut tree = Worktree {
            git: self.clone(),
            top: place.top.clone(),
            path: place.path.clone(),
            inside: place.path.clone(),
            released: false,
        };
        let added = self.add(&place, commit);
        match added.and_then(|()| inside(&place, commit)) {
            Ok(inside) => {
                tree.inside = inside;
                Ok(tree)
            }
            Err(error) => Err(WorktreeFailure {
                error,
                cleanup: tree.release(),
            }),
        }
    }

    fn place(&self, root: &Path, cache_root: &Path) -> Result<Place, WorktreeError> {
        let resolved = resolve_path(root)?;
        let Some(top) = self.root(&resolved) else {
            return Err(WorktreeError::NotRepository { resolved });
        };
        let scratch = resolve_path(&cache_root.join(WORKTREE_DIR))?;
        if scratch.starts_with(&top) {
            return Err(WorktreeError::ScratchInside { scratch, top });
        }
        io::create_owner_dir(&scratch).map_err(|error| os_error(&scratch, error))?;
        let path = scratch.join(random_name()?);
        Ok(Place {
            resolved,
            top,
            path,
        })
    }

    fn add(&self, place: &Place, commit: &str) -> Result<(), WorktreeError> {
        let rest = ["add", "--detach"]
            .map(OsStr::new)
            .into_iter()
            .chain([place.path.as_os_str(), OsStr::new(commit)])
            .collect::<Vec<_>>();
        let call = Call {
            config: &NO_REPO_CODE,
            subcommand: "worktree",
            rest: &rest,
        };
        let _lock = lock();
        let added = self.run_git(&place.top, &call, CREATE);
        added.map(drop).map_err(WorktreeError::Add)
    }

    fn run_git(&self, cwd: &Path, call: &Call<'_>, limits: Limits) -> Result<Vec<u8>, RunGitError> {
        checked(call.subcommand, cwd, self.run_call(cwd, call, limits))
    }
}

// Every answer but a full stdout is an error: the write side has no use for half a diff.
pub(crate) fn checked(
    subcommand: &'static str,
    cwd: &Path,
    outcome: GitOutcome,
) -> Result<Vec<u8>, RunGitError> {
    let cwd = cwd.to_path_buf();
    match outcome {
        GitOutcome::Printed(stdout) => Ok(stdout),
        GitOutcome::Capped(_) => Err(RunGitError::Capped { subcommand, cwd }),
        GitOutcome::Failed(failure) => Err(RunGitError::Failed {
            note: failure.to_string(),
            cwd,
        }),
        GitOutcome::Unfinished(unfinished) => Err(RunGitError::Failed {
            note: unfinished.to_string(),
            cwd,
        }),
    }
}

fn resolve_path(path: &Path) -> Result<PathBuf, WorktreeError> {
    io::resolve(path).map_err(|error| os_error(path, error))
}

fn os_error(path: &Path, error: std::io::Error) -> WorktreeError {
    let path = path.to_path_buf();
    WorktreeError::Os { path, error }
}

fn random_name() -> Result<String, WorktreeError> {
    let mut bytes = [0; NAME_BYTES];
    io::random_bytes(&mut bytes).map_err(|error| os_error(Path::new("/dev/urandom"), error))?;
    let digits = b"0123456789abcdef";
    let hex: String = bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .map(|digit| char::from(digits[usize::from(digit)]))
        .collect();
    Ok(format!("wt-{hex}"))
}

// The directory of the worktree that corresponds to the root; a root that HEAD lacks has none.
fn inside(place: &Place, commit: &str) -> Result<PathBuf, WorktreeError> {
    let Ok(relative) = place.resolved.strip_prefix(&place.top) else {
        return Err(WorktreeError::OutsideTop {
            resolved: place.resolved.clone(),
            top: place.top.clone(),
        });
    };
    let inside = if relative.as_os_str().is_empty() {
        place.path.clone()
    } else {
        place.path.join(relative)
    };
    if inside.is_dir() {
        return Ok(inside);
    }
    Err(WorktreeError::NotInCommit {
        resolved: place.resolved.clone(),
        commit: commit.to_owned(),
        top: place.top.clone(),
        relative: relative.to_path_buf(),
    })
}

fn lock() -> MutexGuard<'static, ()> {
    WORKTREE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Worktree {
    /// Returns the directory in the worktree that corresponds to the root of the call.
    #[must_use]
    pub fn inside(&self) -> &Path {
        &self.inside
    }

    /// Returns the worktree directory, `<scratch>/wt-<32 hex digits>`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the worktree and prunes the repository's record of it, and returns the notes of
    /// each step that failed.
    #[must_use]
    pub fn release(mut self) -> Vec<CleanupNote> {
        self.released = true;
        release(&self.git, &self.top, &self.path)
    }
}

impl fmt::Debug for Worktree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worktree")
            .field("top", &self.top)
            .field("path", &self.path)
            .field("inside", &self.inside)
            .finish_non_exhaustive()
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if !self.released {
            let _lost = release(&self.git, &self.top, &self.path);
        }
    }
}

// `--force` twice: a killed creation leaves its record locked, and one force keeps a locked
// worktree. A failed remove falls back to removing the directory.
pub(crate) fn release(git: &Git, top: &Path, path: &Path) -> Vec<CleanupNote> {
    let _lock = lock();
    let mut notes = Vec::new();
    let remove = ["remove", "--force", "--force"]
        .map(OsStr::new)
        .into_iter()
        .chain([path.as_os_str()])
        .collect::<Vec<_>>();
    if let Err(error) = git.run_git(top, &worktree_call(&remove), CLEANUP) {
        let path = path.to_path_buf();
        // The existence check below reports what this removal leaves.
        let _ = io::remove_tree(&path);
        notes.push(CleanupNote::RemoveFailed { path, error });
    }
    if let Err(error) = git.run_git(top, &worktree_call(&[OsStr::new("prune")]), CLEANUP) {
        let root = top.to_path_buf();
        notes.push(CleanupNote::PruneFailed { root, error });
    }
    if path.exists() {
        let path = path.to_path_buf();
        notes.push(CleanupNote::Leftover { path });
    }
    notes
}

fn worktree_call<'a>(rest: &'a [&'a OsStr]) -> Call<'a> {
    Call {
        config: &[],
        subcommand: "worktree",
        rest,
    }
}

impl fmt::Display for RunGitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capped { subcommand, cwd } => write!(
                f,
                "git {subcommand} printed more than {MAX_GIT_OUTPUT_BYTES} bytes in {}",
                cwd.display()
            ),
            Self::Failed { note, cwd } => write!(f, "{note} (in {})", cwd.display()),
        }
    }
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCommit { root } => write!(
                f,
                "{} is not inside a git repository, or HEAD names no commit, so no worktree can \
                 be created for it.",
                root.display()
            ),
            Self::Git(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for CleanupNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RemoveFailed { path, error } => {
                let path = path.display();
                write!(
                    f,
                    "git worktree remove failed for {path}: {error}; removing the directory"
                )
            }
            Self::PruneFailed { root, error } => {
                write!(
                    f,
                    "git worktree prune failed in {}: {error}",
                    root.display()
                )
            }
            Self::Leftover { path } => {
                let path = path.display();
                let tail = "could not be removed and is still on disk; delete it by hand";
                write!(f, "scratch worktree {path} {tail}")
            }
        }
    }
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRepository { resolved } => write!(
                f,
                "{} is not inside a git repository, so no worktree can be created for it. Patch \
                 apply needs a git repository; run it against one, or use --in-place.",
                resolved.display()
            ),
            Self::ScratchInside { scratch, top } => write!(
                f,
                "scratch worktrees would be created at {}, inside the repository {} they are \
                 meant to stay out of. Point XDG_CACHE_HOME at a directory outside the \
                 repository.",
                scratch.display(),
                top.display()
            ),
            Self::NotInCommit {
                resolved,
                commit,
                top,
                relative,
            } => write!(
                f,
                "{} is not in commit {commit} of {}, so the worktree has no {} to work in. The \
                 directory is untracked or ignored: commit it, or point this call at a directory \
                 the repository tracks.",
                resolved.display(),
                top.display(),
                relative.display()
            ),
            Self::OutsideTop { resolved, top } => write!(
                f,
                "{} is not in the subpath of {}",
                quoted(resolved),
                quoted(top)
            ),
            Self::Add(error) => error.fmt(f),
            Self::Os { path, error } => write!(f, "{}: {}", io::os_error_line(error), quoted(path)),
        }
    }
}

fn quoted(path: &Path) -> String {
    CodePointString::from_utf8_escaped(path.as_os_str().as_encoded_bytes()).quoted()
}

impl std::error::Error for RunGitError {}
impl std::error::Error for PinError {}
impl std::error::Error for WorktreeError {}

impl From<RunGitError> for kernel::Error {
    fn from(error: RunGitError) -> Self {
        Self::OperationFailed(error.to_string())
    }
}

impl From<PinError> for kernel::Error {
    fn from(error: PinError) -> Self {
        match error {
            PinError::NoCommit { .. } => Self::RepoResolution(error.to_string()),
            PinError::Git(error) => error.into(),
        }
    }
}

impl From<WorktreeError> for kernel::Error {
    fn from(error: WorktreeError) -> Self {
        let message = error.to_string();
        match error {
            WorktreeError::NotRepository { .. }
            | WorktreeError::ScratchInside { .. }
            | WorktreeError::NotInCommit { .. } => Self::RepoResolution(message),
            WorktreeError::OutsideTop { .. } | WorktreeError::Add(_) | WorktreeError::Os { .. } => {
                Self::OperationFailed(message)
            }
        }
    }
}

#[cfg(test)]
mod tests;
