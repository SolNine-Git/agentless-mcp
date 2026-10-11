use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{Failure, repo, verdict};

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    path: Option<PathBuf>,
}

type Layers = BTreeMap<String, u8>;

/// Fail when a crate under `crates/` has no layer, or depends on a crate in its own or a higher layer.
pub fn check(root: &Path) -> Result<(), Failure> {
    let layers: Layers = repo::toml(&root.join("crates/layers.toml"))?;
    let packages = workspace_crates(root)?;
    verdict(
        packages
            .iter()
            .flat_map(|package| problems(package, &layers))
            .collect(),
    )
}

fn workspace_crates(root: &Path) -> Result<Vec<Package>, Failure> {
    let mut command = Command::new("cargo");
    command
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root);
    let output = repo::stdout(&mut command)?;
    let metadata: Metadata = serde_json::from_slice(&output)
        .map_err(|error| Failure::Broken(format!("cargo metadata: {error}")))?;
    let crates = root.join("crates");
    let inside = |package: &Package| package.manifest_path.starts_with(&crates);
    Ok(metadata.packages.into_iter().filter(inside).collect())
}

fn problems(package: &Package, layers: &Layers) -> Vec<String> {
    let Some(&own) = layers.get(&package.name) else {
        return vec![format!("{} has no row in crates/layers.toml", package.name)];
    };
    let local = package
        .dependencies
        .iter()
        .filter(|dependency| dependency.path.is_some());
    local
        .filter_map(|dependency| match layers.get(&dependency.name) {
            None => Some(format!(
                "{} depends on {}, which has no row in crates/layers.toml",
                package.name, dependency.name
            )),
            Some(&layer) if layer >= own => Some(format!(
                "{} (layer {own}) depends on {} (layer {layer}): a crate depends only on lower layers",
                package.name, dependency.name
            )),
            Some(_) => None,
        })
        .collect()
}
