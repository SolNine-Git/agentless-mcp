//! The child-process port, and the linter runs of `linter_delta`: the config search, the
//! program, and one bounded run of each text of a pair.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use checks::{LintedPair, LintedSides, LinterFailure, LinterFinding, NotLinted, Pair};
use kernel::{Deadline, walk_budget};
use prompts::Linter;

use crate::Services;

/// Starts a child process and waits for its outcome: the port that every linter run goes through.
pub trait ChildRunner: Send + Sync {
    /// Runs `spec` once.
    fn run(&self, spec: &io::Spec) -> io::Outcome;
}

/// The runner that starts a real child process with [`io::run`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

impl ChildRunner for SystemRunner {
    fn run(&self, spec: &io::Spec) -> io::Outcome {
        io::run(spec)
    }
}

// A run gets at most this long, and at most half of the time the call has left.
const RUN_CAP: Duration = Duration::from_secs(2);
const STDOUT_CAP: usize = 16 << 20;
const STDERR_CAP: usize = 1 << 16;
const PATH_SLOT: &str = "{path}";

// What every linter run of one lint call reads.
#[derive(Clone, Copy)]
pub(crate) struct Linting<'a> {
    pub(crate) root: &'a Path,
    pub(crate) runner: &'a dyn ChildRunner,
    pub(crate) env: &'a [(OsString, OsString)],
    pub(crate) git: &'a git::Git,
    pub(crate) deadline: Option<&'a Deadline>,
}

impl<'a> Linting<'a> {
    pub(crate) fn of(
        services: &'a Services,
        root: &'a Path,
        deadline: Option<&'a Deadline>,
    ) -> Self {
        Self {
            root,
            runner: &*services.runner,
            env: &services.linter_env,
            git: &services.git,
            deadline,
        }
    }
}

// The linter row of the language of `path`, when the language has one.
pub(crate) fn linter_of(path: &str) -> Option<&'static Linter> {
    let language = checks::pair_language(path)?;
    let rules = (prompts::linter_rules().iter()).find(|rules| rules.language == language.name())?;
    rules.linter.as_ref().ok()
}

pub(crate) fn has_linter(path: &str) -> bool {
    linter_of(path).is_some()
}

// Each pair with a new text in a language with a linter, linted on both of its texts.
pub(crate) fn linted<'p>(pairs: &[&'p Pair], linting: &Linting<'_>) -> Vec<LintedPair<'p>> {
    let mut linted = Vec::new();
    for pair in pairs {
        let (Some(new), Some(linter)) = (pair.new_text(), linter_of(pair.path())) else {
            continue;
        };
        linted.push(LintedPair {
            pair,
            linter,
            linted: lint_pair(pair, new, linter, linting),
        });
    }
    linted
}

// The old text runs first, and a failure there leaves the new text unrun.
fn lint_pair(
    pair: &Pair,
    new: &str,
    linter: &Linter,
    linting: &Linting<'_>,
) -> Result<LintedSides, NotLinted> {
    let path = pair.path();
    if !has_config(linting.root, path, linter) {
        return Err(NotLinted::NoConfig);
    }
    let program = program(linting, linter);
    let run = |text: &str| run_once(linting, linter, (&program, path), text);
    let old = match pair.old_text() {
        Some(old) => run(old)?,
        None => Vec::new(),
    };
    Ok(LintedSides {
        old,
        new: run(new)?,
    })
}

fn run_once(
    linting: &Linting<'_>,
    linter: &Linter,
    (program, path): (&OsString, &str),
    text: &str,
) -> Result<Vec<LinterFinding>, NotLinted> {
    let timeout = walk_budget(RUN_CAP, linting.deadline.map(Deadline::remaining));
    if timeout.is_zero() {
        return Err(NotLinted::Failed(LinterFailure::BudgetSpent));
    }
    let args = (linter.args.iter())
        .map(|arg| OsString::from(arg.replace(PATH_SLOT, path)))
        .collect();
    let spec = io::Spec {
        program: program.clone(),
        args,
        cwd: linting.root.to_path_buf(),
        env: linting.env.to_vec(),
        timeout,
        stdout_cap: STDOUT_CAP,
        stderr_cap: STDERR_CAP,
        stdin: Some(text.as_bytes().to_vec()),
    };
    checks::ran(linting.runner.run(&spec), timeout, STDOUT_CAP)
}

// The repository's own program when no tracked content can have put it there, else the bare
// name, which the run finds on PATH.
fn program(linting: &Linting<'_>, linter: &Linter) -> OsString {
    if untracked_inside(linting.git, linting.root, linter.local) {
        linting.root.join(linter.local).into_os_string()
    } else {
        OsString::from(linter.program)
    }
}

// A file that resolves inside the root, with no tracked entry at its top folder or below it (a
// symlink or a submodule there included) or at its target. A git that cannot answer gives false.
fn untracked_inside(git: &git::Git, root: &Path, local: &str) -> bool {
    let named = Path::new(local);
    let program = root.join(named);
    if !program.is_file() {
        return false;
    }
    let Some(target) = resolved_below(root, &program) else {
        return false;
    };
    let untracked = |path: &Path| git.tracks(root, path) == Ok(false);
    let folder = named.iter().next().map_or(named, Path::new);
    untracked(folder) && (target == named || untracked(&target))
}

// The path below the root of the file that `path` resolves to, when that file is inside the root.
fn resolved_below(root: &Path, path: &Path) -> Option<PathBuf> {
    let top = io::resolve(root).ok()?;
    let resolved = io::contained_path(&top, path).ok()?;
    resolved.strip_prefix(&top).ok().map(Path::to_path_buf)
}

// A config in the folder of `path` or in a folder above it, up to the root.
fn has_config(root: &Path, path: &str, linter: &Linter) -> bool {
    let folder = Path::new(path).parent().unwrap_or(Path::new(""));
    let mut folders = folder.ancestors();
    folders.any(|folder| names_config(&root.join(folder), linter))
}

fn names_config(folder: &Path, linter: &Linter) -> bool {
    let named = |file: &&str| folder.join(file).is_file();
    linter.configs.iter().any(named) || manifest_names(&folder.join(linter.manifest), linter.table)
}

// A manifest that does not read or parse counts as a config, so the linter's run reports it.
fn manifest_names(manifest: &Path, table: &[&str]) -> bool {
    let text = match std::fs::read_to_string(manifest) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match text.parse::<toml::Table>() {
        Ok(document) => holds_key(&document, table),
        Err(_) => true,
    }
}

// Every key but the last names a table, and the last key is in it.
fn holds_key(document: &toml::Table, keys: &[&str]) -> bool {
    let Some((last, outer)) = keys.split_last() else {
        return false;
    };
    let mut table = document;
    for key in outer {
        match table.get(*key) {
            Some(toml::Value::Table(inner)) => table = inner,
            _ => return false,
        }
    }
    table.contains_key(*last)
}

#[cfg(test)]
mod tests;
