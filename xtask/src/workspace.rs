use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use toml::{Table, Value};

use crate::{Failure, process, repo};

/// The directories that hold every workspace package and every file the source gates walk.
pub const ROOTS: [&str; 2] = ["crates", "xtask"];
/// The reviewed list of the third-party packages that the workspace uses.
pub const DEPENDENCIES: &str = "crates/dependencies.toml";
/// The target kind that cargo metadata gives a build script.
pub const BUILD_SCRIPT: &str = "custom-build";
const LIBRARY_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];
// cargo test and nextest build the tests of these kinds, and of the library kinds, unless `test = false`.
const TESTED_KINDS: [&str; 2] = ["bin", "test"];
// A proc macro runs at expansion and a build script before the compile: both run third-party code.
const BUILD_TIME_KINDS: [&str; 2] = ["proc-macro", BUILD_SCRIPT];

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Member>,
}

// The full resolved graph, every platform included, so the list is the same on each CI platform.
#[derive(Deserialize)]
struct Graph {
    packages: Vec<Node>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    name: String,
    targets: Vec<NodeTarget>,
}

#[derive(Deserialize)]
struct NodeTarget {
    kind: Vec<String>,
}

/// The third-party packages that the owner reviewed: the direct dependencies, and the packages that run at build time.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listed {
    pub direct: BTreeSet<String>,
    #[serde(rename = "build-time")]
    pub build_time: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Member {
    name: String,
    manifest_path: PathBuf,
    rust_version: Option<String>,
    dependencies: Vec<Dependency>,
    targets: Vec<Target>,
}

/// A workspace package, as `cargo metadata` reports it.
pub struct Package {
    pub name: String,
    pub dir: PathBuf,
    pub rust_version: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub targets: Vec<Target>,
}

/// A dependency of a package; `path` is set for a local one.
#[derive(Deserialize)]
pub struct Dependency {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// A build target, the crate root rustc reads for it, and the settings that decide how cargo builds and tests it.
#[derive(Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: PathBuf,
    pub edition: String,
    pub test: bool,
    // cargo metadata omits the key when the target requires no feature.
    #[serde(rename = "required-features")]
    pub required_features: Option<Vec<String>>,
}

/// Return every workspace package that `cargo metadata` lists.
pub fn packages(root: &Path) -> Result<Vec<Package>, Failure> {
    let mut command = Command::new("cargo");
    command
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root);
    let output = process::stdout(&mut command, process::METADATA)?;
    let metadata: Metadata = serde_json::from_slice(&output)
        .map_err(|error| Failure::Broken(format!("cargo metadata: {error}")))?;
    metadata.packages.into_iter().map(package).collect()
}

/// Read the reviewed list of third-party packages.
pub fn listed(root: &Path) -> Result<Listed, Failure> {
    repo::toml(&root.join(DEPENDENCIES))
}

/// Return the text of `key` in the root manifest's [workspace.package], which every package inherits.
pub fn package_key(manifest: &Table, key: &str) -> Result<String, Failure> {
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get(key))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::Broken(format!("Cargo.toml has no [workspace.package] {key}")))
}

/// Return the package name of each direct dependency of the workspace packages, path dependencies left out.
pub fn direct(packages: &[Package]) -> BTreeSet<String> {
    packages
        .iter()
        .flat_map(|package| &package.dependencies)
        .filter(|dependency| dependency.path.is_none())
        .map(|dependency| dependency.name.clone())
        .collect()
}

/// Return each package outside the workspace in the resolved graph that has a proc-macro or build-script target.
pub fn build_time(root: &Path) -> Result<BTreeSet<String>, Failure> {
    let mut command = Command::new("cargo");
    command
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
        ])
        .current_dir(root);
    let output = process::stdout(&mut command, process::METADATA)?;
    let graph: Graph = serde_json::from_slice(&output)
        .map_err(|error| Failure::Broken(format!("cargo metadata: {error}")))?;
    Ok(build_time_packages(&graph))
}

/// Return a problem for each name in `found` that the list's `key` lacks, and each name it holds that `found` lacks.
pub fn list_problems(
    key: &str,
    what: &str,
    found: &BTreeSet<String>,
    listed: &BTreeSet<String>,
) -> Vec<String> {
    let unlisted = found.difference(listed).map(|name| {
        format!(
            "{name} is {what}, and {DEPENDENCIES} does not list it in `{key}`: its code reaches the build unreviewed. After review, add the name in a commit with a `Policy-Change: {DEPENDENCIES}` trailer"
        )
    });
    let stale = listed.difference(found).map(|name| {
        format!(
            "{DEPENDENCIES} lists {name} in `{key}`, and it is not {what}. Remove the name in a commit with a `Policy-Change: {DEPENDENCIES}` trailer"
        )
    });
    unlisted.chain(stale).collect()
}

fn build_time_packages(graph: &Graph) -> BTreeSet<String> {
    graph
        .packages
        .iter()
        .filter(|node| !graph.workspace_members.contains(&node.id))
        .filter(|node| {
            node.targets
                .iter()
                .flat_map(|target| &target.kind)
                .any(|kind| BUILD_TIME_KINDS.contains(&kind.as_str()))
        })
        .map(|node| node.name.clone())
        .collect()
}

fn package(member: Member) -> Result<Package, Failure> {
    let dir = member.manifest_path.parent().ok_or_else(|| {
        Failure::Broken(format!(
            "cargo metadata: {} has no directory",
            member.manifest_path.display()
        ))
    })?;
    Ok(Package {
        name: member.name,
        dir: dir.to_path_buf(),
        rust_version: member.rust_version,
        dependencies: member.dependencies,
        targets: member.targets,
    })
}

impl Package {
    /// Return the library target, if the package has one.
    pub fn library(&self) -> Option<&Target> {
        self.targets.iter().find(|target| target.is_library())
    }
}

impl Target {
    /// Return whether the target is a library of any crate type.
    pub fn is_library(&self) -> bool {
        self.kind
            .iter()
            .any(|kind| LIBRARY_KINDS.contains(&kind.as_str()))
    }

    /// Return whether cargo builds the target's tests when the manifest does not set `test`.
    pub fn tested_by_default(&self) -> bool {
        self.is_library()
            || self
                .kind
                .iter()
                .any(|kind| TESTED_KINDS.contains(&kind.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::{
        Dependency, Graph, Listed, Metadata, Package, build_time_packages, direct, list_problems,
        package_key,
    };
    use crate::Failure;

    fn names(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_direct_set_is_each_dependency_without_a_path_by_package_name() {
        let dependency = |name: &str, path: Option<&str>| Dependency {
            name: name.to_owned(),
            path: path.map(PathBuf::from),
        };
        let package = |dependencies| Package {
            name: "p".to_owned(),
            dir: PathBuf::from("/r/crates/p"),
            rust_version: None,
            dependencies,
            targets: Vec::new(),
        };
        let packages = [
            package(vec![
                dependency("serde", None),
                dependency("kernel", Some("/r/crates/kernel")),
            ]),
            package(vec![dependency("serde", None), dependency("toml", None)]),
        ];
        assert_eq!(direct(&packages), names(&["serde", "toml"]));
    }

    #[test]
    fn the_build_time_set_is_each_proc_macro_or_build_script_package_outside_the_workspace() {
        let json = r#"{"workspace_members": ["path+file:///r/crates/kernel#0.1.0"], "packages": [
            {"id": "path+file:///r/crates/kernel#0.1.0", "name": "kernel", "targets": [{"kind": ["custom-build"]}]},
            {"id": "registry+x#serde_derive@1.0.0", "name": "serde_derive", "targets": [{"kind": ["proc-macro"]}]},
            {"id": "registry+x#serde@1.0.0", "name": "serde", "targets": [{"kind": ["lib"]}, {"kind": ["custom-build"]}]},
            {"id": "registry+x#itoa@1.0.0", "name": "itoa", "targets": [{"kind": ["lib"]}]}]}"#;
        let graph: Graph = serde_json::from_str(json).unwrap();
        assert_eq!(
            build_time_packages(&graph),
            names(&["serde", "serde_derive"])
        );
    }

    #[test]
    fn an_unlisted_or_a_stale_name_is_a_problem_and_the_list_refuses_unknown_keys() {
        let problems = list_problems(
            "direct",
            "a direct dependency",
            &names(&["itoa", "serde"]),
            &names(&["serde", "toml"]),
        );
        assert_eq!(problems.len(), 2);
        assert!(problems[0].starts_with(
            "itoa is a direct dependency, and crates/dependencies.toml does not list it in `direct`"
        ));
        assert!(problems[1].starts_with(
            "crates/dependencies.toml lists toml in `direct`, and it is not a direct dependency"
        ));
        assert!(
            problems
                .iter()
                .all(|problem| problem.contains("Policy-Change: crates/dependencies.toml"))
        );
        let listed: Listed = toml::from_str("direct = [\"a\"]\nbuild-time = []\n").unwrap();
        assert_eq!(listed.direct, names(&["a"]));
        assert!(toml::from_str::<Listed>("direct = []\nbuild-time = []\nallow = []\n").is_err());
        assert!(toml::from_str::<Listed>("direct = []\n").is_err());
    }

    #[test]
    fn a_workspace_package_key_is_read_as_text_or_is_broken() {
        let key = |text: &str| package_key(&toml::from_str(text).unwrap(), "edition");
        let manifest = "[workspace]\nmembers = []\n[workspace.package]\nedition = \"2024\"\n";
        assert_eq!(key(manifest).unwrap(), "2024");
        for text in [
            "[workspace]\nmembers = []\n",
            "[workspace.package]\nedition = 2024\n",
        ] {
            assert!(
                matches!(key(text), Err(Failure::Broken(message)) if message == "Cargo.toml has no [workspace.package] edition"),
                "{text}"
            );
        }
    }

    #[test]
    fn a_member_that_lacks_a_field_fails_the_parse_and_names_it() {
        for (json, field) in [
            (
                r#"{"packages": [{"name": "k", "manifest_path": "/r/k/Cargo.toml", "dependencies": []}]}"#,
                "targets",
            ),
            (
                r#"{"packages": [{"name": "k", "manifest_path": "/r/k/Cargo.toml", "deps": [], "targets": []}]}"#,
                "dependencies",
            ),
            (
                r#"{"packages": [{"name": "k", "manifest_path": "/r/k/Cargo.toml", "dependencies": [],
                    "targets": [{"name": "k", "kind": ["lib"], "src_path": "/r/k/src/lib.rs", "test": true}]}]}"#,
                "edition",
            ),
            (
                r#"{"packages": [{"name": "k", "manifest_path": "/r/k/Cargo.toml", "dependencies": [],
                    "targets": [{"name": "k", "kind": ["lib"], "src_path": "/r/k/src/lib.rs", "edition": "2024"}]}]}"#,
                "test",
            ),
        ] {
            let error = serde_json::from_str::<Metadata>(json).err().unwrap();
            assert!(error.to_string().contains(field), "{error}");
        }
    }
}
