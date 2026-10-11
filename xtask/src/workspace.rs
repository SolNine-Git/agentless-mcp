use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{Failure, process};

/// The directories that hold every workspace package and every file the source gates walk.
pub const ROOTS: [&str; 2] = ["crates", "xtask"];
const LIBRARY_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];
// cargo test and nextest build the tests of these kinds, and of the library kinds, unless `test = false`.
const TESTED_KINDS: [&str; 2] = ["bin", "test"];

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Member>,
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
    use super::Metadata;

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
