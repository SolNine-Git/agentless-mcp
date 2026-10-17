//! The repository write: every file of a patch is written, or the tree is left as it was.
//!
//! Three phases. Each new content goes to an exclusively created sibling with the target's mode.
//! Backup names are then reserved the same way. Last, each target moves to its backup and its
//! staged file moves into its place. A failed move restores the completed targets from their
//! backups in reverse order. SIGINT is held for the whole write.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::files::sync_written;
use crate::native::close_checked;
use crate::random::{create_named, sibling_prefix};
use crate::{Refusal, WatchError, contained_path, hold_sigint, remove_file, strerror};

/// The suffix of a staged file until every file of the patch is staged.
pub const STAGING_SUFFIX: &str = ".agentless-mcp-staged";
/// The suffix of an original file while its replacement moves into place.
pub const BACKUP_SUFFIX: &str = ".agentless-mcp-backup";

/// A temporary sibling that could not be removed. It displays as the warning about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    /// The sibling that stays on disk.
    pub path: PathBuf,
    /// The operating system's message.
    pub reason: String,
}

/// A write that failed, and the leftovers that its cleanup found first.
#[derive(Debug)]
pub struct WriteFailure {
    /// Why the write failed.
    pub error: WriteError,
    /// The siblings that the cleanup could not remove, in the order it met them.
    pub leftovers: Vec<Leftover>,
}

/// Why [`write_all`] failed. It displays as the error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// A path resolves outside the base directory.
    Refused(Refusal),
    /// The staged file for `path` could not be written. Nothing changed.
    Stage {
        /// The path as the caller named it.
        path: String,
        /// The operating system's message.
        reason: String,
    },
    /// A backup name could not be reserved. Nothing changed.
    Reserve {
        /// The operating system's message.
        reason: String,
    },
    /// A target could not be replaced, and every original was restored.
    Restored {
        /// The file name of the target.
        name: OsString,
        /// The operating system's message.
        reason: String,
    },
    /// A target could not be replaced, and the restore of some originals failed too.
    PartlyApplied {
        /// The file name of the target.
        name: OsString,
        /// The operating system's message.
        reason: String,
        /// The backups that hold the originals, in the order of the restore.
        retained: Vec<PathBuf>,
    },
    /// The SIGINT watch could not be registered. Nothing changed.
    Watch {
        /// The operating system's message.
        reason: String,
    },
}

/// Writes each `(path, bytes)` pair below `base`, all or none, in path order. A later pair for a
/// path replaces an earlier one. Each path must resolve inside `base`. The returned leftovers are
/// the backups that could not be removed after the write.
///
/// # Errors
///
/// Returns the [`WriteFailure`] of the first step that failed, after the cleanup.
pub fn write_all<P: AsRef<str>, B: AsRef<[u8]>>(
    base: &Path,
    contents: &[(P, B)],
) -> Result<Vec<Leftover>, WriteFailure> {
    write_held(&Disk, base, contents)
}

// A SIGINT during the write waits until every file is written or every original is restored.
pub(crate) fn write_held<P: AsRef<str>, B: AsRef<[u8]>>(
    steps: &impl Steps,
    base: &Path,
    contents: &[(P, B)],
) -> Result<Vec<Leftover>, WriteFailure> {
    let _hold =
        hold_sigint().map_err(|WatchError(error)| failure(watch_error(&error), Vec::new()))?;
    write_with(steps, base, contents)
}

fn watch_error(error: &io::Error) -> WriteError {
    WriteError::Watch {
        reason: strerror(error),
    }
}

// The file operations of the write. Tests replace them to fail at a chosen step.
pub(crate) trait Steps {
    // Creates a sibling of `target` that holds `bytes` with the target's mode, and returns it.
    fn stage(&self, target: &Path, bytes: &[u8]) -> io::Result<PathBuf>;
    // Creates an empty sibling of `target` for its backup, and returns it.
    fn reserve(&self, target: &Path) -> io::Result<PathBuf>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    // Removes `path`; a missing file is no error.
    fn remove(&self, path: &Path) -> io::Result<()>;
}

pub(crate) struct Disk;

impl Steps for Disk {
    // The flush makes a deferred write error a stage failure; stage_file checks the close too.
    fn stage(&self, target: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        stage_file(target, bytes, |file, bytes, name| {
            write_bytes(file, bytes, name)?;
            sync_written(file)
        })
    }

    fn reserve(&self, target: &Path) -> io::Result<PathBuf> {
        let (directory, prefix) = sibling_parts(target);
        create_named(directory, &prefix, BACKUP_SUFFIX).map(|(path, _file)| path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        remove_file(path).map(drop)
    }
}

pub(crate) fn write_with<P: AsRef<str>, B: AsRef<[u8]>>(
    steps: &impl Steps,
    base: &Path,
    contents: &[(P, B)],
) -> Result<Vec<Leftover>, WriteFailure> {
    let files: BTreeMap<&str, &[u8]> = contents
        .iter()
        .map(|(path, bytes)| (path.as_ref(), bytes.as_ref()))
        .collect();
    let staged = stage_all(steps, base, &files)?;
    let backups = reserve_all(steps, &staged)?;
    replace_all(steps, &staged, &backups)?;
    Ok(discard(steps, backups.iter()))
}

// A staged file and its target.
struct Staged {
    staging: PathBuf,
    target: PathBuf,
}

fn stage_all(
    steps: &impl Steps,
    base: &Path,
    files: &BTreeMap<&str, &[u8]>,
) -> Result<Vec<Staged>, WriteFailure> {
    let mut staged: Vec<Staged> = Vec::new();
    for (&path, &bytes) in files {
        let staging = contained_path(base, Path::new(path))
            .map_err(WriteError::Refused)
            .and_then(|target| stage_one(steps, path, target, bytes));
        match staging {
            Ok(one) => staged.push(one),
            Err(error) => return Err(failure(error, discard_staged(steps, &staged))),
        }
    }
    Ok(staged)
}

fn stage_one(
    steps: &impl Steps,
    path: &str,
    target: PathBuf,
    bytes: &[u8],
) -> Result<Staged, WriteError> {
    match steps.stage(&target, bytes) {
        Ok(staging) => Ok(Staged { staging, target }),
        Err(error) => Err(WriteError::Stage {
            path: path.to_owned(),
            reason: strerror(&error),
        }),
    }
}

fn reserve_all(steps: &impl Steps, staged: &[Staged]) -> Result<Vec<PathBuf>, WriteFailure> {
    let mut backups = Vec::new();
    for one in staged {
        match steps.reserve(&one.target) {
            Ok(backup) => backups.push(backup),
            Err(error) => {
                let mut leftovers = discard_staged(steps, staged);
                leftovers.extend(discard(steps, backups.iter()));
                let reason = strerror(&error);
                return Err(failure(WriteError::Reserve { reason }, leftovers));
            }
        }
    }
    Ok(backups)
}

fn replace_all(
    steps: &impl Steps,
    staged: &[Staged],
    backups: &[PathBuf],
) -> Result<(), WriteFailure> {
    let mut completed: Vec<(&Path, &Path)> = Vec::new();
    for (position, (one, backup)) in staged.iter().zip(backups).enumerate() {
        let moved = steps.rename(&one.target, backup).and_then(|()| {
            completed.push((backup, &one.target));
            steps.rename(&one.staging, &one.target)
        });
        if let Err(error) = moved {
            let undone = Undone {
                staged,
                backups,
                completed: &completed,
                position,
            };
            return Err(undo(steps, &undone, &error));
        }
    }
    Ok(())
}

// The state of the replace phase when a move failed at `position`.
struct Undone<'a> {
    staged: &'a [Staged],
    backups: &'a [PathBuf],
    completed: &'a [(&'a Path, &'a Path)],
    position: usize,
}

fn undo(steps: &impl Steps, undone: &Undone<'_>, error: &io::Error) -> WriteFailure {
    let retained = roll_back(steps, undone.completed);
    let mut leftovers = discard_staged(steps, &undone.staged[undone.position..]);
    let moved: Vec<&Path> = undone.completed.iter().map(|(backup, _)| *backup).collect();
    let unused = undone
        .backups
        .iter()
        .filter(|backup| !moved.contains(&backup.as_path()));
    leftovers.extend(discard(steps, unused));
    let target = undone.staged[undone.position].target.as_path();
    let name = target.file_name().unwrap_or(OsStr::new("")).to_owned();
    let reason = strerror(error);
    let error = if retained.is_empty() {
        WriteError::Restored { name, reason }
    } else {
        WriteError::PartlyApplied {
            name,
            reason,
            retained,
        }
    };
    failure(error, leftovers)
}

// Moves each completed backup back to its target, last first, and returns the backups that
// could not move.
fn roll_back(steps: &impl Steps, completed: &[(&Path, &Path)]) -> Vec<PathBuf> {
    completed
        .iter()
        .rev()
        .filter(|(backup, target)| steps.rename(backup, target).is_err())
        .map(|(backup, _)| backup.to_path_buf())
        .collect()
}

fn discard_staged(steps: &impl Steps, staged: &[Staged]) -> Vec<Leftover> {
    discard(steps, staged.iter().map(|one| &one.staging))
}

// Removes temporary siblings that will never move into place, and returns each that stays.
fn discard<'a>(steps: &impl Steps, paths: impl Iterator<Item = &'a PathBuf>) -> Vec<Leftover> {
    paths
        .filter_map(|path| {
            steps.remove(path).err().map(|error| Leftover {
                path: path.clone(),
                reason: strerror(&error),
            })
        })
        .collect()
}

fn failure(error: WriteError, leftovers: Vec<Leftover>) -> WriteFailure {
    WriteFailure { error, leftovers }
}

// The directory of `target` and the name prefix `.<name>.` of its siblings.
fn sibling_parts(target: &Path) -> (&Path, OsString) {
    let directory = target.parent().unwrap_or(Path::new("/"));
    let prefix = sibling_prefix(target.file_name().unwrap_or(OsStr::new("")));
    (directory, prefix)
}

// The staged file is removed when a step after its creation fails.
pub(crate) fn stage_file(
    target: &Path,
    bytes: &[u8],
    write: impl Fn(&mut File, &[u8], &OsStr) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let (directory, prefix) = sibling_parts(target);
    let (staging, mut file) = create_named(directory, &prefix, STAGING_SUFFIX)?;
    let name = target.file_name().unwrap_or(OsStr::new(""));
    let written = fs::metadata(target)
        .map(|metadata| Permissions::from_mode(metadata.permissions().mode() & 0o7777))
        .and_then(|mode| file.set_permissions(mode))
        .and_then(|()| write(&mut file, bytes, name));
    // The close is checked too: a deferred write error can come only at the close.
    let closed = close_checked(file);
    match written.and(closed) {
        Ok(()) => Ok(staging),
        Err(error) => {
            // The write's error is the one returned; a failed removal leaves a hidden sibling.
            let _ = fs::remove_file(&staging);
            Err(error)
        }
    }
}

// A write that makes no progress is an error, not a short file.
pub(crate) fn write_bytes(out: &mut impl Write, bytes: &[u8], name: &OsStr) -> io::Result<()> {
    let mut rest = bytes;
    while !rest.is_empty() {
        match out.write(rest) {
            Ok(0) => {
                let message = format!(
                    "write returned zero bytes for {}",
                    Path::new(name).display()
                );
                return Err(io::Error::other(message));
            }
            Ok(count) => rest = &rest[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl fmt::Display for Leftover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { path, reason } = self;
        let path = path.display();
        write!(
            f,
            "could not remove the temporary file {path}: {reason}; delete it by hand"
        )
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stage { path, reason } => {
                let tail = "nothing was changed";
                not_applied(f, format_args!("cannot write {path}"), reason, tail)
            }
            Self::Reserve { reason } => {
                let what = format_args!("cannot reserve rollback files");
                not_applied(f, what, reason, "nothing changed")
            }
            Self::Restored { name, reason } => {
                let what = format_args!("cannot replace {}", Path::new(name).display());
                not_applied(f, what, reason, "every original was restored")
            }
            Self::Watch { reason } => {
                let what = format_args!("cannot hold SIGINT");
                not_applied(f, what, reason, "nothing was changed")
            }
            Self::PartlyApplied {
                name,
                reason,
                retained,
            } => write!(
                f,
                "patch partly applied: cannot replace {}: {reason}; rollback failed; originals \
                 retained at: {}",
                Path::new(name).display(),
                joined(retained)
            ),
            Self::Refused(refusal) => refusal.fmt(f),
        }
    }
}

// The form of every failure that changed nothing, or that restored every original.
fn not_applied(
    f: &mut fmt::Formatter<'_>,
    what: fmt::Arguments<'_>,
    reason: &str,
    outcome: &str,
) -> fmt::Result {
    write!(f, "patch not applied: {what}: {reason}; {outcome}")
}

impl std::error::Error for WriteError {}

impl From<WriteError> for kernel::Error {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::Refused(refusal) => refusal.into(),
            other => Self::OperationFailed(other.to_string()),
        }
    }
}

fn joined(paths: &[PathBuf]) -> String {
    let shown: Vec<String> = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    shown.join(", ")
}

#[cfg(test)]
mod tests;
