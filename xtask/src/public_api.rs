use std::path::Path;
use std::process::Command;

use crate::{Failure, repo, verdict};

// cargo-public-api reads rustdoc JSON, which only a nightly toolchain writes. CI installs this one too.
const TOOLCHAIN: &str = "nightly-2026-10-01";
const SNAPSHOT: &str = "public-api.txt";

/// Fail when a library crate's public API differs from its committed snapshot; `--update` rewrites it.
pub fn check(root: &Path, flags: &[String]) -> Result<(), Failure> {
    let update = match flags {
        [] => false,
        [flag] if flag == "--update" => true,
        _ => return Err(Failure::Usage),
    };
    let mut problems = Vec::new();
    for dir in repo::crate_dirs(root)? {
        if !dir.join("src/lib.rs").is_file() {
            continue;
        }
        let name = repo::relative(&dir, root);
        let current = public_api(root, &dir)?;
        let snapshot = dir.join(SNAPSHOT);
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

fn public_api(root: &Path, dir: &Path) -> Result<String, Failure> {
    let package = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::Broken(format!("{} has no crate name", dir.display())))?;
    let mut command = Command::new("cargo");
    command
        .args([&format!("+{TOOLCHAIN}"), "public-api", "--package", package])
        .args(["--simplified", "--color", "never"])
        .current_dir(root);
    let output = repo::stdout(&mut command)?;
    String::from_utf8(output).map_err(|error| Failure::Broken(format!("cargo public-api: {error}")))
}
