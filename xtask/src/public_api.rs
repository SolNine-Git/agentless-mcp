use std::path::Path;
use std::process::Command;

use crate::workspace::{self, Package};
use crate::{Failure, process, repo, verdict};

// cargo-public-api reads rustdoc JSON, which only a nightly toolchain writes. CI installs this one too.
pub const TOOLCHAIN: &str = "nightly-2026-10-01";
const SNAPSHOT: &str = "public-api.txt";

/// Fail when a library crate's public API differs from its committed snapshot; `--update` rewrites it.
pub fn check(root: &Path, flags: &[String]) -> Result<(), Failure> {
    let update = match flags {
        [] => false,
        [flag] if flag == "--update" => true,
        _ => return Err(Failure::Usage),
    };
    let packages = workspace::packages(root)?;
    let mut problems = Vec::new();
    for package in libraries(&packages) {
        let name = repo::relative(&package.dir, root);
        let current = public_api(root, &package.name)?;
        let snapshot = package.dir.join(SNAPSHOT);
        if update {
            repo::write(&snapshot, &current)?;
        } else if !snapshot.is_file() || repo::read(&snapshot)? != current {
            problems.push(format!(
                "{name}/{SNAPSHOT} does not match the public API; review the change, then run cargo xtask public-api --update"
            ));
        }
    }
    verdict(problems)
}

fn libraries(packages: &[Package]) -> impl Iterator<Item = &Package> {
    packages
        .iter()
        .filter(|package| package.library().is_some())
}

fn public_api(root: &Path, package: &str) -> Result<String, Failure> {
    let mut command = Command::new("cargo");
    command
        .args([&format!("+{TOOLCHAIN}"), "public-api", "--package", package])
        .args(["--simplified", "--color", "never"])
        .current_dir(root);
    let output = process::stdout(&mut command, process::BUILD)?;
    String::from_utf8(output).map_err(|error| Failure::Broken(format!("cargo public-api: {error}")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::libraries;
    use crate::workspace::{Package, Target};

    fn package(name: &str, kind: &str, src_path: &str) -> Package {
        Package {
            name: name.to_owned(),
            dir: PathBuf::from(format!("/r/{name}")),
            rust_version: None,
            dependencies: Vec::new(),
            targets: vec![Target {
                name: name.to_owned(),
                kind: vec![kind.to_owned()],
                src_path: PathBuf::from(src_path),
                edition: "2024".to_owned(),
                test: true,
                required_features: None,
            }],
        }
    }

    #[test]
    fn libraries_come_from_the_lib_target() {
        let packages = [
            package("kernel", "lib", "/r/kernel/src/root.rs"),
            package("deep", "rlib", "/r/app/nested/deep/src/lib.rs"),
            package("xtask", "bin", "/r/xtask/src/main.rs"),
        ];
        let names: Vec<&str> = libraries(&packages)
            .map(|package| package.name.as_str())
            .collect();
        assert_eq!(names, ["kernel", "deep"]);
    }
}
