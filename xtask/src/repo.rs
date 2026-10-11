use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;

use crate::{Failure, process};

/// Return the repository root: the directory above this crate, as `cargo run` reports it.
pub fn root() -> Result<PathBuf, Failure> {
    // Read at run time: a compiled-in path goes stale when a shared target dir serves a moved tree.
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .ok_or_else(|| Failure::Broken("run this through cargo xtask".to_owned()))?;
    let above = Path::new(&manifest_dir).join("..");
    std::fs::canonicalize(&above)
        .map_err(|error| Failure::Broken(format!("{}: {error}", above.display())))
}

/// Return `path` relative to `root`, with forward slashes.
pub fn relative(path: &Path, root: &Path) -> String {
    let inside = path.strip_prefix(root).unwrap_or(path);
    inside.to_string_lossy().replace('\\', "/")
}

/// Read a file as text, naming it in the failure.
pub fn read(path: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|error| broken(path, &error))
}

/// Write a file, naming it in the failure.
pub fn write(path: &Path, text: &str) -> Result<(), Failure> {
    std::fs::write(path, text).map_err(|error| broken(path, &error))
}

/// Parse a TOML file, naming it in the failure.
pub fn toml<T: DeserializeOwned>(path: &Path) -> Result<T, Failure> {
    toml::from_str(&read(path)?).map_err(|error| broken(path, &error))
}

/// Return whether `path` exists, without following a symlink; any error but "not found" is a failure.
pub fn exists(path: &Path) -> Result<bool, Failure> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(broken(path, &error)),
    }
}

/// Return the failure for an `error` on `path`, naming the path.
pub fn broken(path: &Path, error: &dyn std::fmt::Display) -> Failure {
    Failure::Broken(format!("{}: {error}", path.display()))
}

/// Run git in `root` under its deadline and return its stdout; a non-zero exit is a failure.
pub fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, Failure> {
    let mut command = Command::new("git");
    command.args(args).current_dir(root);
    process::stdout(&mut command, process::GIT)
}

/// Resolve `revision` to a commit id; with none, take the merge base of HEAD and origin/main.
pub fn base(root: &Path, revision: Option<&str>) -> Result<String, Failure> {
    let revision = match revision {
        Some(revision) => revision.to_owned(),
        None => text(&git(root, &["merge-base", "HEAD", "origin/main"])?),
    };
    let commit = format!("{revision}^{{commit}}");
    let verified = git(
        root,
        &["rev-parse", "--verify", "--end-of-options", &commit],
    )?;
    Ok(text(&verified))
}

/// Return the file at `path` in `revision`, or None when that commit has no such file.
pub fn show(root: &Path, revision: &str, path: &str) -> Result<Option<Vec<u8>>, Failure> {
    let listed = git(root, &["ls-tree", "--name-only", revision, "--", path])?;
    if listed.is_empty() {
        return Ok(None);
    }
    git(root, &["show", &format!("{revision}:{path}")]).map(Some)
}

/// List the commits in `base..HEAD` that touched `path`, merges included, each after its parents.
pub fn touching(root: &Path, base: &str, path: &str) -> Result<Vec<String>, Failure> {
    let range = format!("{base}..HEAD");
    let output = git(
        root,
        &[
            "log",
            "--full-history",
            "--reverse",
            "--topo-order",
            "--format=%H",
            &range,
            "--",
            path,
        ],
    )?;
    Ok(text(&output).lines().map(str::to_owned).collect())
}

fn text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_owned()
}
