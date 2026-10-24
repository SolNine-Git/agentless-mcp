//! The revision form: each file changed since a commit, compared with its working-tree text.

use std::collections::BTreeMap;
use std::path::Path;

use checks::{PairFinding, TextPair};
use git::{Change, GitOutcome, Side};
use prompts::messages::{
    LINT_REVISION_BINARY, LINT_REVISION_BINARY_EVIDENCE, LINT_REVISION_LISTING_CAPPED,
    LINT_REVISION_NOT_RESOLVED, LINT_REVISION_OUTPUT_MALFORMED, LINT_REVISION_STATUS,
    LINT_REVISION_STATUS_EVIDENCE, LINT_REVISION_SUBMODULE, LINT_REVISION_SUBMODULE_EVIDENCE,
    LINT_REVISION_UNCHANGED, LINT_REVISION_UNCHANGED_EVIDENCE, LINT_REVISION_UNLISTED,
    LINT_REVISION_UNRESOLVED, PAIR_BLOB_OVER_CAP, PAIR_OLD_UNREAD, PAIR_WORKTREE_UNREAD,
};

use crate::rows::{Gap, bounded_read, coverage, refused_row, side_text};
use crate::{Error, parallel};

// Each old side is one git process. More than four at a time read no faster.
const MAX_BLOB_READS: usize = 4;

// What the revision form compares: the coverage rows, sorted by path, and the text pairs.
pub(crate) struct Compared {
    pub(crate) rows: Vec<PairFinding>,
    pub(crate) pairs: Vec<TextPair>,
}

// One path to compare: the commit side, the working-tree side, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PairSpec {
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
}

pub(crate) fn compared(git: &git::Git, root: &Path, revision: &str) -> Result<Compared, Error> {
    let commit = match commit_from(revision, git.verify(root, revision))? {
        Ok(commit) => commit,
        Err(row) => return Ok(alone(*row)),
    };
    let changes = match changes(git, root, &commit)? {
        Ok(changes) => changes,
        Err(row) => return Ok(alone(*row)),
    };
    if changes.is_empty() {
        let message = LINT_REVISION_UNCHANGED.render(revision);
        let evidence = LINT_REVISION_UNCHANGED_EVIDENCE.to_owned();
        return Ok(alone(coverage(message, "", evidence)));
    }
    let (mut rows, specs) = pair_specs(&changes);
    let threads = parallel::threads().min(MAX_BLOB_READS);
    let read = parallel::map_in_order(&specs, threads, |spec| pair(git, root, &commit, spec));
    let mut pairs = Vec::new();
    for result in read {
        match result {
            Ok(pair) => pairs.push(pair),
            Err(row) => rows.push(*row),
        }
    }
    rows.sort_by(|left, right| left.finding.path.cmp(&right.finding.path));
    Ok(Compared { rows, pairs })
}

fn alone(row: PairFinding) -> Compared {
    Compared {
        rows: vec![row],
        pairs: Vec::new(),
    }
}

// The commit `revision` names, a row when git gave no answer, or the refusal of a revision that
// names no commit.
pub(crate) fn commit_from(
    revision: &str,
    outcome: GitOutcome,
) -> Result<Result<String, Gap>, Error> {
    match outcome {
        GitOutcome::Printed(stdout) | GitOutcome::Capped(stdout) => git::parse_commit(&stdout)
            .map(Ok)
            .map_err(|detail| malformed(&detail)),
        GitOutcome::Failed(failure) => {
            let message = LINT_REVISION_UNRESOLVED.render((revision, &failure.to_string()));
            Err(kernel::Error::RevisionUnresolved(message).into())
        }
        GitOutcome::Unfinished(unfinished) => {
            let note = unfinished.to_string();
            let message = LINT_REVISION_NOT_RESOLVED.render((revision, &note));
            Ok(Err(Box::new(coverage(message, "", note))))
        }
    }
}

fn malformed(detail: &git::Malformed) -> Error {
    let message = LINT_REVISION_OUTPUT_MALFORMED.render(&detail.to_string());
    kernel::Error::OperationFailed(message).into()
}

// The tracked changes first, then the untracked files.
fn changes(git: &git::Git, root: &Path, commit: &str) -> Result<Result<Vec<Change>, Gap>, Error> {
    let listed = match listing_from("diff", git.changes(root, commit)) {
        Ok(listed) => listed,
        Err(row) => return Ok(Err(row)),
    };
    let untracked = match listing_from("ls-files", git.untracked(root)) {
        Ok(untracked) => untracked,
        Err(row) => return Ok(Err(row)),
    };
    let mut changes = git::parse_changes(&listed).map_err(|detail| malformed(&detail))?;
    let linked = git::parse_untracked(&untracked)
        .into_iter()
        .map(|change| linked(root, change));
    changes.extend(linked);
    Ok(Ok(changes))
}

// A listing's bytes, or the row that says why there is none.
pub(crate) fn listing_from(command: &str, outcome: GitOutcome) -> Result<Vec<u8>, Gap> {
    let note = match outcome {
        GitOutcome::Printed(stdout) => return Ok(stdout),
        GitOutcome::Capped(_) => {
            let message = LINT_REVISION_LISTING_CAPPED.render((command, git::MAX_LISTING_BYTES));
            let evidence = format!("more than {} bytes", git::MAX_LISTING_BYTES);
            return Err(Box::new(coverage(message, "", evidence)));
        }
        GitOutcome::Failed(failure) => failure.to_string(),
        GitOutcome::Unfinished(unfinished) => unfinished.to_string(),
    };
    let message = LINT_REVISION_UNLISTED.render((command, &note));
    Err(Box::new(coverage(message, "", note)))
}

// Git lists an untracked symlink as a plain name, so the link is found here.
fn linked(root: &Path, mut change: Change) -> Change {
    if change.new == Side::File && root.join(&change.path).is_symlink() {
        change.new = Side::Link;
    }
    change
}

// The rows of the changes no pair compares, and the specs of the rest, sorted by path.
pub(crate) fn pair_specs(changes: &[Change]) -> (Vec<PairFinding>, Vec<PairSpec>) {
    let mut rows = Vec::new();
    let mut specs: BTreeMap<String, PairSpec> = BTreeMap::new();
    for change in changes {
        match uncompared(change) {
            Some(row) => rows.push(row),
            None => {
                for spec in change_specs(change) {
                    insert_spec(&mut specs, spec);
                }
            }
        }
    }
    (rows, specs.into_values().collect())
}

// The row of a submodule change or of a status that no pair compares.
fn uncompared(change: &Change) -> Option<PairFinding> {
    if change.old == Side::Gitlink || change.new == Side::Gitlink {
        let message = LINT_REVISION_SUBMODULE.render(&change.path);
        let evidence = LINT_REVISION_SUBMODULE_EVIDENCE.to_owned();
        return Some(coverage(message, &change.path, evidence));
    }
    if git::COMPARED_STATUSES.contains(&change.status) {
        return None;
    }
    let status = change.status.to_string();
    let message = LINT_REVISION_STATUS.render((&change.path, &status));
    let evidence = LINT_REVISION_STATUS_EVIDENCE.render(&status);
    Some(coverage(message, &change.path, evidence))
}

fn insert_spec(specs: &mut BTreeMap<String, PairSpec>, spec: PairSpec) {
    let spec = match specs.remove(&spec.path) {
        Some(held) => joined(held, spec),
        None => spec,
    };
    specs.insert(spec.path.clone(), spec);
}

// A symlink side or a side no pair check reads counts as absent, so a change of language is a
// deletion and an addition.
fn change_specs(change: &Change) -> Vec<PairSpec> {
    let old_language = side_language(change.old, &change.old_path);
    let new_language = side_language(change.new, &change.path);
    if old_language.is_some() && old_language == new_language {
        return vec![PairSpec {
            path: change.path.clone(),
            old_path: Some(change.old_path.clone()),
            new_path: Some(change.path.clone()),
        }];
    }
    let old = old_language.map(|_| PairSpec {
        path: change.old_path.clone(),
        old_path: Some(change.old_path.clone()),
        new_path: None,
    });
    let new = new_language.map(|_| PairSpec {
        path: change.path.clone(),
        old_path: None,
        new_path: Some(change.path.clone()),
    });
    old.into_iter().chain(new).collect()
}

fn side_language(side: Side, path: &str) -> Option<syntax::Language> {
    (side == Side::File)
        .then(|| checks::pair_language(path))
        .flatten()
}

// A deletion and an addition at one path, as `git rm --cached` leaves them.
fn joined(held: PairSpec, spec: PairSpec) -> PairSpec {
    PairSpec {
        path: spec.path,
        old_path: held.old_path.or(spec.old_path),
        new_path: held.new_path.or(spec.new_path),
    }
}

fn pair(git: &git::Git, root: &Path, commit: &str, spec: &PairSpec) -> Result<TextPair, Gap> {
    let old = spec
        .old_path
        .as_ref()
        .map(|path| blob_from(git.blob(root, commit, path)));
    let new = match &spec.new_path {
        Some(path) => Some(worktree_read(root, path)?),
        None => None,
    };
    if old.as_ref().is_some_and(binary) || new.as_ref().is_some_and(binary) {
        let message = LINT_REVISION_BINARY.render(&spec.path);
        let evidence = LINT_REVISION_BINARY_EVIDENCE.render(git::BINARY_PROBE_BYTES);
        return Err(Box::new(coverage(message, &spec.path, evidence)));
    }
    let old = old
        .map(|read| side_text(&PAIR_OLD_UNREAD, &spec.path, read))
        .transpose()?;
    let new = new
        .map(|read| side_text(&PAIR_WORKTREE_UNREAD, &spec.path, read))
        .transpose()?;
    Ok(TextPair {
        path: spec.path.clone(),
        old,
        new,
    })
}

// A blob's bytes, or the reason the side was not read.
pub(crate) fn blob_from(outcome: GitOutcome) -> Result<Vec<u8>, String> {
    match outcome {
        GitOutcome::Printed(stdout) => Ok(stdout),
        GitOutcome::Capped(_) => Err(PAIR_BLOB_OVER_CAP.render(io::MAX_FILE_BYTES)),
        GitOutcome::Failed(failure) => Err(failure.to_string()),
        GitOutcome::Unfinished(unfinished) => Err(unfinished.to_string()),
    }
}

fn worktree_read(root: &Path, path: &str) -> Result<Result<Vec<u8>, String>, Gap> {
    match io::contained_path(root, Path::new(path)) {
        Ok(target) => Ok(bounded_read(&target)),
        Err(refusal) => Err(Box::new(refused_row(&refusal))),
    }
}

// A skipped read is not binary.
fn binary(read: &Result<Vec<u8>, String>) -> bool {
    read.as_ref().is_ok_and(|data| git::looks_binary(data))
}

#[cfg(test)]
mod tests;
