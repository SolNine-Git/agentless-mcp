use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;

use crate::{Failure, process};

// gitinfo.py's HARDENING_PREFIX, log.follow (it drops merges from `git log -- <path>`), and no
// replace ref or commit-graph file (each can rewrite a commit). A diff also passes --no-ext-diff.
const GIT_OPTIONS: [&str; 16] = [
    "--no-optional-locks",
    "--no-replace-objects",
    "-c",
    "core.commitGraph=false",
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
];
// They move where config is read from, not which repository is read (gitinfo.py GIT_CONFIG_KEPT).
const GIT_CONFIG_KEPT: [&str; 2] = ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM"];
// A graft file rewrites parents as a replace ref does. Nothing exists below /dev/null, and git is
// silent when the open fails with ENOTDIR.
const NO_GRAFTS: &str = "/dev/null/grafts";

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

/// Return a git command in `root` with the fixed options, no graft file, and without the inherited GIT_ variables but the config ones.
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
    command.env("GIT_GRAFT_FILE", NO_GRAFTS);
    command
}

/// Resolve `revision` to a commit id; with none, take the merge base of HEAD and origin/main.
pub fn base(root: &Path, revision: Option<&str>) -> Result<String, Failure> {
    // A shallow entry cuts a commit's parents, so the range from the base would lose commits.
    if text(&git(root, &["rev-parse", "--is-shallow-repository"])?) != "false" {
        return Err(Failure::Broken(
            "the repository is shallow, so git cannot list every commit since the base. Fetch the full history: git fetch --unshallow".to_owned(),
        ));
    }
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
pub(crate) mod fixture {
    use std::path::{Path, PathBuf};

    use super::{git_command, text};
    use crate::process;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // It reads no user or system config, so no user hook runs and the identity is fixed.
    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        let mut command = git_command(dir);
        command
            .args(["-c", "user.name=t", "-c", "user.email=t@invalid"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        text(&process::stdout(&mut command, process::GIT).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    use super::fixture::{git, scratch};
    use super::{base, fixed_git, text, touching};
    use crate::Failure;

    fn commit(dir: &Path, file: &str, content: &str, message: &str) -> String {
        std::fs::write(dir.join(file), content).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn touching_lists_an_evil_merge_whatever_log_follow_the_repository_sets() {
        let dir = scratch("repo-follow");
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "log.follow", "true"]);
        let base = commit(&dir, "clippy.toml", "a = 60\n", "base");
        git(&dir, &["checkout", "-q", "-b", "side"]);
        commit(&dir, "other", "x\n", "side");
        git(&dir, &["checkout", "-q", "main"]);
        let tighten = commit(&dir, "clippy.toml", "a = 59\n", "tighten");
        git(&dir, &["merge", "-q", "--no-ff", "--no-commit", "side"]);
        let merge = commit(&dir, "clippy.toml", "a = 1000\n", "merge");
        let listed = touching(&dir, &base, "clippy.toml");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(listed.unwrap(), [tighten, merge]);
    }

    #[test]
    fn a_graft_file_does_not_change_the_parents_that_git_reports() {
        let dir = scratch("repo-graft");
        git(&dir, &["init", "-q", "-b", "main"]);
        let first = commit(&dir, "a", "1\n", "one");
        let second = commit(&dir, "a", "2\n", "two");
        let third = commit(&dir, "a", "3\n", "three");
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        std::fs::write(dir.join(".git/info/grafts"), format!("{third} {first}\n")).unwrap();
        let parents = super::git(&dir, &["log", "-1", "--format=%P", &third]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(text(&parents.unwrap()), second);
    }

    #[test]
    fn a_shallow_repository_has_no_base() {
        let dir = scratch("repo-shallow");
        git(&dir, &["init", "-q", "-b", "main"]);
        let first = commit(&dir, "a", "1\n", "one");
        let second = commit(&dir, "a", "2\n", "two");
        let whole = base(&dir, Some(&first));
        std::fs::write(dir.join(".git/shallow"), format!("{second}\n")).unwrap();
        let shallow = base(&dir, Some(&first));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(whole.unwrap(), first);
        assert!(
            matches!(&shallow, Err(Failure::Broken(message)) if message.contains("shallow")),
            "{shallow:?}"
        );
    }

    // Overwrite the first parent of `commit` in the commit-graph file. git reads the file without
    // checking its checksum.
    fn forge_first_parent(dir: &Path, commit: &str, parent: &str) {
        let path = dir.join(".git/objects/info/commit-graph");
        let mut graph = std::fs::read(&path).unwrap();
        let chunk = |id: &[u8]| {
            let entry = (0..usize::from(graph[6]))
                .map(|index| &graph[8 + 12 * index..20 + 12 * index])
                .find(|entry| &entry[..4] == id)
                .unwrap();
            usize::try_from(u64::from_be_bytes(entry[4..].try_into().unwrap())).unwrap()
        };
        let (fanout, ids, data) = (chunk(b"OIDF"), chunk(b"OIDL"), chunk(b"CDAT"));
        let count = u32::from_be_bytes(graph[fanout + 1020..fanout + 1024].try_into().unwrap());
        let position = |hex: &str| {
            let id: Vec<u8> = (0..40)
                .step_by(2)
                .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
                .collect();
            (0..usize::try_from(count).unwrap())
                .position(|index| graph[ids + 20 * index..ids + 20 * (index + 1)] == id[..])
                .unwrap()
        };
        let entry = data + 36 * position(commit);
        let forged = u32::try_from(position(parent)).unwrap().to_be_bytes();
        graph[entry + 20..entry + 24].copy_from_slice(&forged);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, graph).unwrap();
    }

    #[test]
    fn a_commit_graph_file_does_not_change_the_parents_that_git_reports() {
        let dir = scratch("repo-commit-graph");
        git(&dir, &["init", "-q", "-b", "main"]);
        let first = commit(&dir, "a", "1\n", "one");
        let second = commit(&dir, "a", "2\n", "two");
        let third = commit(&dir, "a", "3\n", "three");
        let graph = ["-c", "core.commitGraph=true"];
        git(
            &dir,
            &[&graph[..], &["commit-graph", "write", "--reachable"]].concat(),
        );
        forge_first_parent(&dir, &third, &first);
        let parents = ["log", "-1", "--format=%P", third.as_str()];
        let forged = git(&dir, &[&graph[..], &parents].concat());
        let reported = super::git(&dir, &parents);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(forged, first, "the forged graph is not in use");
        assert_eq!(text(&reported.unwrap()), second);
    }

    #[test]
    fn git_drops_each_inherited_git_variable_but_the_config_ones() {
        let inherited = [
            "GIT_DIR",
            "GIT_CONFIG_GLOBAL",
            "PATH",
            "GIT_CONFIG_COUNT",
            "GIT_GRAFT_FILE",
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
        let grafts = command
            .get_envs()
            .find(|(name, _)| *name == "GIT_GRAFT_FILE");
        assert_eq!(
            grafts,
            Some((
                OsStr::new("GIT_GRAFT_FILE"),
                Some(OsStr::new("/dev/null/grafts"))
            ))
        );
        assert_eq!(command.get_envs().count(), 3);
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args[0], "--no-optional-locks");
        assert!(args.contains(&OsStr::new("log.follow=false")));
        assert!(args.contains(&OsStr::new("--no-replace-objects")));
        assert!(args.contains(&OsStr::new("core.commitGraph=false")));
    }
}
