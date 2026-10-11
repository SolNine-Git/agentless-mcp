use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{Failure, process};

const LIBRARY_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Member>,
}

#[derive(Deserialize)]
struct Member {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
    targets: Vec<Target>,
}

/// A workspace package, as `cargo metadata` reports it.
pub struct Package {
    pub name: String,
    pub dir: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub targets: Vec<Target>,
}

/// A dependency of a package; `path` is set for a local one.
#[derive(Deserialize)]
pub struct Dependency {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// A build target and the crate root rustc reads for it.
#[derive(Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: PathBuf,
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
        dependencies: member.dependencies,
        targets: member.targets,
    })
}

impl Package {
    /// Return the library target, if the package has one.
    pub fn library(&self) -> Option<&Target> {
        self.targets.iter().find(|target| {
            target
                .kind
                .iter()
                .any(|kind| LIBRARY_KINDS.contains(&kind.as_str()))
        })
    }
}
