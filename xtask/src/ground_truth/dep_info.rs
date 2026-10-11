use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::repo;
use crate::sources::Source;
use crate::tool_config::CLIPPY_NAMES;
use crate::workspace::Package;

const MANIFEST: &str = "Cargo.toml";

/// The files that rustc may read for a workspace target: the scanned sources, the manifests, and clippy configs.
pub struct Allowed {
    root: PathBuf,
    files: BTreeSet<PathBuf>,
}

impl Allowed {
    /// Allow each scanned source, the root manifest and each package manifest.
    pub fn new(root: &Path, sources: &[Source], packages: &[Package]) -> Self {
        let sources = sources.iter().map(|source| root.join(&source.file));
        let manifests = packages.iter().map(|package| package.dir.join(MANIFEST));
        Self {
            root: root.to_path_buf(),
            files: sources
                .chain(manifests)
                .chain([root.join(MANIFEST)])
                .collect(),
        }
    }

    // clippy-driver lists the clippy config that it read. The tool-config gate decides which may exist.
    fn allows(&self, path: &Path) -> bool {
        let clippy = path.starts_with(&self.root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| CLIPPY_NAMES.contains(&name));
        clippy || self.files.contains(path)
    }
}

/// Return each path that a dep-info file lists, resolved against the root without touching the file system.
pub fn paths(text: &str, root: &Path) -> BTreeSet<PathBuf> {
    // After the rule of each output, rustc writes a rule `<input>:` for each input, with `\ ` for a space.
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_suffix(':'))
        .map(|path| normalize(&root.join(path.replace("\\ ", " "))))
        .collect()
}

/// Return a problem for each path that rustc reads for `target` and that `allowed` does not hold.
pub fn problems(
    root: &Path,
    target: &Path,
    paths: &BTreeSet<PathBuf>,
    allowed: &Allowed,
) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !allowed.allows(path))
        .map(|path| {
            format!(
                "{}: rustc reads {}, which is not a .rs file that the gates scan: an include!, include_str!, include_bytes! or #[path] names it, directly or through a macro. Move the code into a module under crates/ or xtask/, or read the data at run time",
                repo::relative(target, root),
                repo::relative(path, root)
            )
        })
        .collect()
}

fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    normal
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::{Allowed, paths, problems};
    use crate::sources::Source;
    use crate::tokens;
    use crate::workspace::Package;

    const DEP_INFO: &str =
        "/r/target/debug/deps/xtask-f9.d: /r/xtask/clippy.toml xtask/src/main.rs \
xtask/src/../../crates/kernel/src/id.rs my\\ notes.md Cargo.toml

/r/target/debug/deps/libxtask-f9.rmeta: /r/xtask/clippy.toml xtask/src/main.rs

/r/xtask/clippy.toml:
xtask/src/main.rs:
xtask/src/../../crates/kernel/src/id.rs:
my\\ notes.md:
Cargo.toml:

# env-dep:CARGO_MANIFEST_DIR=/r/xtask
# env-dep:CLIPPY_CONF_DIR
";

    fn allowed() -> Allowed {
        let sources: Vec<Source> = ["xtask/src/main.rs", "crates/kernel/src/id.rs"]
            .iter()
            .map(|file| Source {
                file: (*file).to_owned(),
                scan: tokens::scan("").unwrap(),
            })
            .collect();
        let packages = [Package {
            name: "xtask".to_owned(),
            dir: PathBuf::from("/r/xtask"),
            rust_version: None,
            dependencies: Vec::new(),
            targets: Vec::new(),
        }];
        Allowed::new(Path::new("/r"), &sources, &packages)
    }

    fn set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_input_rules_are_read_with_parent_steps_and_escaped_spaces_resolved() {
        assert_eq!(
            paths(DEP_INFO, Path::new("/r")),
            set(&[
                "/r/xtask/clippy.toml",
                "/r/xtask/src/main.rs",
                "/r/crates/kernel/src/id.rs",
                "/r/my notes.md",
                "/r/Cargo.toml",
            ])
        );
    }

    #[test]
    fn only_scanned_sources_manifests_and_clippy_configs_under_the_root_are_allowed() {
        let read = set(&[
            "/r/xtask/src/main.rs",
            "/r/crates/kernel/src/id.rs",
            "/r/Cargo.toml",
            "/r/xtask/Cargo.toml",
            "/r/xtask/clippy.toml",
            "/r/.clippy.toml",
            "/r/crates/kernel/src/payload.txt",
            "/r/xtask/src/unlisted.rs",
            "/r/crates/kernel/Cargo.toml",
            "/home/u/clippy.toml",
            "/etc/hostname",
        ]);
        let problems = problems(
            Path::new("/r"),
            Path::new("/r/xtask/src/main.rs"),
            &read,
            &allowed(),
        );
        let named: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split(", which").next().unwrap())
            .collect();
        assert_eq!(
            named,
            [
                "xtask/src/main.rs: rustc reads /etc/hostname",
                "xtask/src/main.rs: rustc reads /home/u/clippy.toml",
                "xtask/src/main.rs: rustc reads crates/kernel/Cargo.toml",
                "xtask/src/main.rs: rustc reads crates/kernel/src/payload.txt",
                "xtask/src/main.rs: rustc reads xtask/src/unlisted.rs",
            ]
        );
    }
}
