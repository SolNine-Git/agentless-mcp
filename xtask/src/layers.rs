use std::collections::BTreeMap;
use std::path::Path;

use crate::workspace::{self, Package};
use crate::{Failure, repo, verdict};

type Layers = BTreeMap<String, u8>;

const NO_PACKAGES: &str =
    "cargo metadata lists no package under crates/: layers has nothing to check";

/// Fail when a crate under `crates/` has no layer or depends upward, or a path dependency leaves the workspace.
pub fn check(root: &Path) -> Result<(), Failure> {
    let layers: Layers = repo::toml(&root.join("crates/layers.toml"))?;
    let packages = workspace::packages(root)?;
    verdict(problems(&packages, &root.join("crates"), &layers))
}

fn problems(packages: &[Package], crates: &Path, layers: &Layers) -> Vec<String> {
    let ranked: Vec<&Package> = packages
        .iter()
        .filter(|package| package.dir.starts_with(crates))
        .collect();
    let mut problems = if ranked.is_empty() {
        vec![NO_PACKAGES.to_owned()]
    } else {
        Vec::new()
    };
    problems.extend(
        ranked
            .iter()
            .flat_map(|package| layer_problems(package, layers)),
    );
    problems.extend(
        packages
            .iter()
            .flat_map(|package| foreign_paths(package, packages)),
    );
    problems
}

fn layer_problems(package: &Package, layers: &Layers) -> Vec<String> {
    let Some(&own) = layers.get(&package.name) else {
        return vec![format!("{} has no row in crates/layers.toml", package.name)];
    };
    // Dev and build dependencies count too: a test or a build script that imports upward breaks the order.
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

fn foreign_paths(package: &Package, packages: &[Package]) -> Vec<String> {
    package
        .dependencies
        .iter()
        .filter_map(|dependency| Some((dependency, dependency.path.as_ref()?)))
        .filter(|(_, path)| !packages.iter().any(|member| member.dir == **path))
        .map(|(dependency, path)| {
            format!(
                "{} depends on {} at {}, which is not a workspace package",
                package.name,
                dependency.name,
                path.display()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Layers, NO_PACKAGES, problems};
    use crate::workspace::{Dependency, Package};

    fn package(name: &str, dir: &str, path_dependencies: &[(&str, &str)]) -> Package {
        let dependencies = path_dependencies
            .iter()
            .map(|&(name, path)| Dependency {
                name: name.to_owned(),
                path: Some(PathBuf::from(path)),
            })
            .collect();
        Package {
            name: name.to_owned(),
            dir: PathBuf::from(dir),
            rust_version: None,
            dependencies,
            targets: Vec::new(),
        }
    }

    fn layers() -> Layers {
        Layers::from([("kernel".to_owned(), 0), ("prompts".to_owned(), 1)])
    }

    #[test]
    fn zero_packages_is_a_problem() {
        let only_xtask = [package("xtask", "/r/xtask", &[])];
        assert_eq!(
            problems(&only_xtask, Path::new("/r/crates"), &layers()),
            [NO_PACKAGES]
        );
    }

    #[test]
    fn a_dependency_on_the_same_or_a_higher_layer_is_a_problem() {
        let packages = [
            package(
                "kernel",
                "/r/crates/kernel",
                &[("prompts", "/r/crates/prompts")],
            ),
            package("prompts", "/r/crates/prompts", &[]),
        ];
        assert_eq!(
            problems(&packages, Path::new("/r/crates"), &layers()),
            ["kernel (layer 0) depends on prompts (layer 1): a crate depends only on lower layers"]
        );
    }

    #[test]
    fn a_path_dependency_outside_the_workspace_is_a_problem() {
        let packages = [
            package("kernel", "/r/crates/kernel", &[]),
            package(
                "prompts",
                "/r/crates/prompts",
                &[("kernel", "/elsewhere/kernel")],
            ),
        ];
        assert_eq!(
            problems(&packages, Path::new("/r/crates"), &layers()),
            ["prompts depends on kernel at /elsewhere/kernel, which is not a workspace package"]
        );
    }
}
