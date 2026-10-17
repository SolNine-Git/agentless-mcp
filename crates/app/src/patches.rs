//! The patch operations over one repository: check, apply and normalize, and the edits loader.
//!
//! Every path an edit names resolves inside the repository root before anything opens it, and
//! the outcomes name the root-relative form. Apply writes all of a patch or none of it.

mod edits;
mod reports;
mod sources;
mod worktree;

use std::collections::BTreeMap;
use std::path::Path;

use patch::{ApplyResult, Edit, apply_edits};
use syntax::Language;

use crate::{Error, Noted, Repo, Services};
use sources::{read_sources, with_read_reasons};

pub(crate) use worktree::{Scratch, every_ignored};

pub use edits::load_edits;
pub use reports::{ApplyReport, CheckReport, FileCheck, NormalizeReport};
pub use sources::{NO_SUCH_FILE, NOT_UTF8};

const INTERRUPTED: &str =
    "patch apply was interrupted by SIGINT after its write; no diff was taken";
// A worktree apply never writes the working tree, and its worktree is released.
const INTERRUPTED_IN_WORKTREE: &str = "patch apply was interrupted by SIGINT; no diff was taken";

/// Where apply writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    /// A scratch worktree at the commit the receipt names.
    Worktree,
    /// The working tree, which must be clean.
    InPlace,
}

/// Applies `edits` in memory to the working tree and reports the syntax delta of each edited file.
///
/// # Errors
///
/// Returns a security refusal for a path outside the root, and an operation failure when a
/// parser refuses a text.
pub fn check_patch(repo: &Repo, edits: &[Edit]) -> Result<CheckReport, Error> {
    let canonical = canonical(&repo.root, edits).map_err(kernel::Error::from)?;
    let sources = read_sources(&repo.root, &canonical).map_err(kernel::Error::from)?;
    let result = apply_edits(&canonical, &sources.contents);
    let mut files: Vec<FileCheck> = sources
        .unreadable
        .iter()
        .map(|(path, unread)| FileCheck {
            path: path.clone(),
            verdict: None,
            error: unread.reason.clone(),
        })
        .collect();
    let mut changed = changes(&sources.contents, &result);
    changed.sort_unstable_by_key(|(path, _, _)| *path);
    for (path, old, new) in changed {
        let verdict = patch::syntax_delta(old, new, Language::from_path(path));
        files.push(FileCheck {
            path: path.to_owned(),
            verdict: Some(verdict.map_err(|error| unparsable(path, &error))?),
            error: String::new(),
        });
    }
    Ok(CheckReport { files, result })
}

/// Returns the equivalence key of the change that `edits` make to the working tree, with the key
/// of each changed file.
///
/// # Errors
///
/// Returns a security refusal for a path outside the root, and an operation failure when a
/// parser refuses a text.
pub fn normalize_patch(repo: &Repo, edits: &[Edit]) -> Result<NormalizeReport, Error> {
    let canonical = canonical(&repo.root, edits).map_err(kernel::Error::from)?;
    let sources = read_sources(&repo.root, &canonical).map_err(kernel::Error::from)?;
    let result = apply_edits(&canonical, &sources.contents);
    let mut file_keys = BTreeMap::new();
    for (path, old, new) in changes(&sources.contents, &result) {
        let key = patch::file_key(old, new, Language::from_path(path));
        file_keys.insert(
            path.to_owned(),
            key.map_err(|error| unparsable(path, &error))?,
        );
    }
    Ok(NormalizeReport {
        key: patch::equivalence_key(&file_keys),
        file_keys,
        result,
    })
}

/// Applies `edits` all or nothing and returns the diff they produce. The worktree mode writes a
/// scratch worktree at the receipt's HEAD; the in-place mode writes the working tree, and refuses
/// a tree that is not provably clean. The warnings are the temporary files the write left, the
/// ignored cache home, and the worktree cleanup notes, with either outcome.
///
/// A SIGINT during the write lets the write finish, and the result is then a stopped call with no
/// diff.
#[must_use]
pub fn apply_patch(
    services: &Services,
    repo: &Repo,
    snapshot: &git::Snapshot,
    edits: &[Edit],
    mode: ApplyMode,
) -> Noted<Result<ApplyReport, Error>> {
    let canonical = match canonical(&repo.root, edits) {
        Ok(canonical) => canonical,
        Err(refusal) => return Noted::new(Err(kernel::Error::from(refusal).into())),
    };
    match mode {
        ApplyMode::InPlace => match require_clean(snapshot.dirty_count) {
            Ok(()) => {
                let target = Target::working_tree(&repo.root);
                applied_at(&services.git, &target, &canonical, io::interrupted)
            }
            Err(refusal) => Noted::new(Err(refusal)),
        },
        ApplyMode::Worktree => in_worktree(services, repo, snapshot, &canonical),
    }
}

// Each path resolves inside the root and is rewritten relative to it, so `./app.py` and
// `app.py` are one file. The one rule that patch and lint share.
pub(crate) fn canonical(root: &Path, edits: &[Edit]) -> Result<Vec<Edit>, io::Refusal> {
    let root = io::resolve(root).map_err(|_| io::Refusal::Unusable)?;
    edits
        .iter()
        .map(|edit| {
            Ok(Edit {
                path: relative(&root, &edit.path)?,
                ..edit.clone()
            })
        })
        .collect()
}

// The resolved root itself is ".". A name that is not UTF-8 shows U+FFFD.
fn relative(root: &Path, path: &str) -> Result<String, io::Refusal> {
    let resolved = io::contained_path(root, Path::new(path))?;
    let Ok(inside) = resolved.strip_prefix(root) else {
        // The root moved between the two resolutions, so containment is not proven.
        return Err(io::Refusal::Outside {
            resolved,
            root: root.to_path_buf(),
        });
    };
    let text = inside.to_string_lossy();
    Ok(if text.is_empty() {
        ".".to_owned()
    } else {
        text.into_owned()
    })
}

// The old and new text of each changed file. apply_edits changes only the files it was given.
fn changes<'a>(
    contents: &'a BTreeMap<String, String>,
    result: &'a ApplyResult,
) -> Vec<(&'a str, &'a str, &'a str)> {
    result
        .new_contents
        .iter()
        .map(|(path, new)| {
            let old = contents.get(path);
            let old = old.expect("apply_edits changes only the files it was given");
            (path.as_str(), old.as_str(), new.as_str())
        })
        .collect()
}

fn unparsable(path: &str, error: &syntax::Unparsable) -> Error {
    kernel::Error::OperationFailed(format!("{path}: {error}")).into()
}

// The count is the snapshot the call started with, so a later write only widens the diff.
fn require_clean(dirty_count: Option<usize>) -> Result<(), Error> {
    let message = match dirty_count {
        Some(0) => return Ok(()),
        None => "--in-place refused: this repository's dirty-file count could not be read, so a \
                 clean tree cannot be proven. Drop --in-place to apply in a worktree."
            .to_owned(),
        Some(count) => format!(
            "--in-place refused: {count} files are modified or untracked. Commit or stash them, \
             or drop --in-place to apply in a scratch worktree."
        ),
    };
    Err(kernel::Error::OperationFailed(message).into())
}

// The apply in a scratch worktree at the commit that the receipt names.
fn in_worktree(
    services: &Services,
    repo: &Repo,
    snapshot: &git::Snapshot,
    edits: &[Edit],
) -> Noted<Result<ApplyReport, Error>> {
    let head = snapshot.head_sha.as_deref();
    let commit = match services.git.pinned_commit(&repo.root, head) {
        Ok(commit) => commit,
        Err(error) => return Noted::new(Err(kernel::Error::from(error).into())),
    };
    let scratch = Scratch {
        services,
        root: &repo.root,
        commit: &commit,
        interrupted: &io::interrupted,
        stopped: || stopped(INTERRUPTED_IN_WORKTREE),
    };
    scratch.run(every_ignored, |tree| {
        let target = Target {
            dir: tree.inside(),
            in_place: false,
            base: format!("HEAD ({})", head.unwrap_or("unknown")),
        };
        applied_at(&services.git, &target, edits, io::interrupted)
    })
}

fn stopped(text: &str) -> Error {
    kernel::Error::CallStopped(text.to_owned()).into()
}

// The directory an apply writes, and how the report names it.
struct Target<'a> {
    dir: &'a Path,
    in_place: bool,
    base: String,
}

impl<'a> Target<'a> {
    fn working_tree(dir: &'a Path) -> Target<'a> {
        Target {
            dir,
            in_place: true,
            base: "the working tree".to_owned(),
        }
    }
}

// Nothing is written unless every edit applied, so a failed patch diffs as the tree it found.
fn applied_at(
    git: &git::Git,
    target: &Target<'_>,
    edits: &[Edit],
    interrupted: impl Fn() -> bool,
) -> Noted<Result<ApplyReport, Error>> {
    let sources = match read_sources(target.dir, edits) {
        Ok(sources) => sources,
        Err(refusal) => return Noted::new(Err(kernel::Error::from(refusal).into())),
    };
    let result = with_read_reasons(apply_edits(edits, &sources.contents), &sources.unreadable);
    let mut diagnostics = Vec::new();
    if result.ok() {
        let written = io::write_all(target.dir, &result.new_contents);
        let leftovers = match &written {
            Ok(leftovers) => leftovers,
            Err(failure) => &failure.leftovers,
        };
        diagnostics.extend(leftovers.iter().map(ToString::to_string));
        if let Err(failure) = written {
            let value = Err(kernel::Error::from(failure.error).into());
            return Noted { value, diagnostics };
        }
        if interrupted() {
            let value = Err(stopped(INTERRUPTED));
            return Noted { value, diagnostics };
        }
    }
    let value = match git.worktree_diff(target.dir) {
        Ok(diff) => Ok(ApplyReport {
            diff,
            result,
            in_place: target.in_place,
            base: target.base.clone(),
        }),
        Err(error) => Err(kernel::Error::from(error).into()),
    };
    Noted { value, diagnostics }
}

#[cfg(test)]
mod tests;
