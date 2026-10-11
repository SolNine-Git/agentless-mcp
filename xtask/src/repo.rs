use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;

use crate::{Failure, process};

// gitinfo.py's HARDENING_PREFIX, so that no config runs a program or changes the output, plus
// log.follow (it drops merges from `git log -- <path>`) and trailer.separators (it widens a trailer).
const GIT_OPTIONS: [&str; 15] = [
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
    "-c",
    "core.quotePath=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "log.follow=false",
    "-c",
    "trailer.separators=:",
];
// They move where config is read from, not which repository is read (gitinfo.py GIT_CONFIG_KEPT).
const GIT_CONFIG_KEPT: [&str; 2] = ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM"];

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
    process::stdout(git_command(root).args(args), process::GIT)
}

/// Return a git command in `root` with the fixed options, and without the inherited GIT_ variables but the config ones.
pub fn git_command(root: &Path) -> Command {
    fixed_git(root, std::env::vars_os().map(|(name, _)| name))
}

fn fixed_git(root: &Path, inherited: impl IntoIterator<Item = OsString>) -> Command {
    let mut command = Command::new("git");
    command.args(GIT_OPTIONS).current_dir(root);
    for name in inherited {
        let text = name.to_string_lossy();
        if text.starts_with("GIT_") && !GIT_CONFIG_KEPT.contains(&text.as_ref()) {
            command.env_remove(&name);
        }
    }
    command
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

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};

    use super::{fixed_git, git_command, text, touching};
    use crate::process;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-repo-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Setup reads no user or system config, so no user hook runs and the identity is fixed.
    fn setup(dir: &Path, args: &[&str]) -> String {
        let mut command = git_command(dir);
        command
            .args(["-c", "user.name=t", "-c", "user.email=t@invalid"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        text(&process::stdout(&mut command, process::GIT).unwrap())
    }

    fn commit(dir: &Path, file: &str, content: &str, message: &str) -> String {
        std::fs::write(dir.join(file), content).unwrap();
        setup(dir, &["add", "-A"]);
        setup(dir, &["commit", "-q", "-m", message]);
        setup(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn touching_lists_an_evil_merge_whatever_log_follow_the_repository_sets() {
        let dir = scratch("follow");
        setup(&dir, &["init", "-q", "-b", "main"]);
        setup(&dir, &["config", "log.follow", "true"]);
        let base = commit(&dir, "clippy.toml", "a = 60\n", "base");
        setup(&dir, &["checkout", "-q", "-b", "side"]);
        commit(&dir, "other", "x\n", "side");
        setup(&dir, &["checkout", "-q", "main"]);
        let tighten = commit(&dir, "clippy.toml", "a = 59\n", "tighten");
        setup(&dir, &["merge", "-q", "--no-ff", "--no-commit", "side"]);
        let merge = commit(&dir, "clippy.toml", "a = 1000\n", "merge");
        let listed = touching(&dir, &base, "clippy.toml");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(listed.unwrap(), [tighten, merge]);
    }

    #[test]
    fn git_drops_each_inherited_git_variable_but_the_config_ones() {
        let inherited = [
            "GIT_DIR",
            "GIT_CONFIG_GLOBAL",
            "PATH",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_NOSYSTEM",
        ]
        .map(OsString::from);
        let command = fixed_git(Path::new("/r"), inherited);
        let removed: Vec<&OsStr> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name)
            .collect();
        assert_eq!(removed, ["GIT_CONFIG_COUNT", "GIT_DIR"]);
        assert_eq!(command.get_envs().count(), 2);
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args[0], "--no-optional-locks");
        assert!(args.contains(&OsStr::new("log.follow=false")));
    }
}
