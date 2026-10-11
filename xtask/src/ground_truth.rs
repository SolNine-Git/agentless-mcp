mod cargo_clippy;
mod coverage;
mod dep_info;
mod lints;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cargo_clippy::{Run, Unit};
use coverage::Ledger;
use dep_info::Allowed;

use crate::workspace::Package;
use crate::{Failure, repo, sources, suppressions, verdict, workspace};

// Apart from the normal target dir: each unit's fingerprint holds CLIPPY_ARGS, so a shared dir would rebuild
// the workspace crates each time this check and the normal clippy run take turns.
const TARGET_DIR: &str = "target/ground-truth";
const MANIFEST: &str = "Cargo.toml";

/// Fail unless clippy, with each warn and deny lint force-warned, finds only suppressions that crates/suppressions.toml lists for the target, rustc reads only scanned sources and allowed data, and the workspace forbids unknown cfgs.
///
/// Residuals: a macro- or derive-built suppression of a lint that the same target already lists. `dead_code` in
/// a bin's test-only code (the test harness allows it on `main`). Lints in external macro output, which clippy
/// and rustc skip. The cfg names that rustc knows (`docsrs`, `miri`, target and panic values). A build script's
/// check-cfg.
pub fn check(root: &Path) -> Result<(), Failure> {
    verdict(problems(root)?)
}

fn problems(root: &Path) -> Result<Vec<String>, Failure> {
    let packages = workspace::packages(root)?;
    let table = lints::table(root)?;
    let configured = lints::configured(root)?;
    let levels = lints::levels(&table, &configured)
        .map_err(|error| Failure::Broken(format!("{MANIFEST}: {error}")))?;
    let mut problems = lints::cfg_problems(&levels, &configured);
    let run = clippy(root, &packages, &lints::force_warned(&levels))?;
    if !run.errors.is_empty() {
        problems.extend(run.errors);
        return Ok(problems);
    }
    let files = dep_info_files(root, &packages, &run.units)?;
    let sources = sources::scan(root, &packages)?;
    let allowed = Allowed::new(root, &sources.files, &packages);
    for (target, paths) in &files {
        problems.extend(dep_info::problems(root, target, paths, &allowed));
    }
    let entries = suppressions::entries(root)?;
    let ledger = Ledger {
        entries: &entries,
        groups: &table.groups,
    };
    let relative = relative_files(root, &files);
    problems.extend(coverage::problems(
        root,
        &run.diagnostics,
        &relative,
        &ledger,
    ));
    Ok(problems)
}

fn clippy(root: &Path, packages: &[Package], forced: &[String]) -> Result<Run, Failure> {
    let manifests: Vec<PathBuf> = packages
        .iter()
        .map(|package| package.dir.join(MANIFEST))
        .collect();
    let mut merged = Run::default();
    for (mode, selection) in cargo_clippy::SELECTIONS {
        let output = cargo_clippy::clippy(root, &root.join(TARGET_DIR), forced, selection)?;
        let run = cargo_clippy::parse(&output, mode, &manifests)?;
        merged.diagnostics.extend(run.diagnostics);
        merged.units.extend(run.units);
        merged.errors.extend(run.errors);
    }
    // The normal and the test unit of one target report the same compile error.
    merged.errors.sort();
    merged.errors.dedup();
    Ok(merged)
}

fn dep_info_files(
    root: &Path,
    packages: &[Package],
    units: &[Unit],
) -> Result<BTreeMap<PathBuf, BTreeSet<PathBuf>>, Failure> {
    let mut files: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    for unit in units {
        let paths = dep_info::paths(&repo::read(&unit.dep_info)?, root);
        if !paths.contains(&unit.target) {
            return Err(Failure::Broken(format!(
                "{}: the dep-info file does not list its crate root {}",
                unit.dep_info.display(),
                unit.target.display()
            )));
        }
        files.entry(unit.target.clone()).or_default().extend(paths);
    }
    let unbuilt = unbuilt(packages, &files);
    if unbuilt.is_empty() {
        Ok(files)
    } else {
        Err(Failure::Broken(format!(
            "cargo clippy built no unit for {}, so the checks would not see it",
            unbuilt.join(", ")
        )))
    }
}

fn unbuilt(packages: &[Package], files: &BTreeMap<PathBuf, BTreeSet<PathBuf>>) -> Vec<String> {
    packages
        .iter()
        .flat_map(|package| &package.targets)
        .filter(|target| !files.contains_key(&target.src_path))
        .map(|target| target.src_path.display().to_string())
        .collect()
}

fn relative_files(
    root: &Path,
    files: &BTreeMap<PathBuf, BTreeSet<PathBuf>>,
) -> BTreeMap<PathBuf, BTreeSet<String>> {
    files
        .iter()
        .map(|(target, paths)| {
            let relative = paths
                .iter()
                .map(|path| repo::relative(path, root))
                .collect();
            (target.clone(), relative)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use super::unbuilt;
    use crate::workspace::{Package, Target};

    fn target(src_path: &str) -> Target {
        Target {
            name: "t".to_owned(),
            kind: vec!["lib".to_owned()],
            src_path: PathBuf::from(src_path),
            edition: "2024".to_owned(),
            test: true,
            required_features: None,
        }
    }

    #[test]
    fn a_target_that_no_unit_built_is_named() {
        let packages = [Package {
            name: "kernel".to_owned(),
            dir: PathBuf::from("/r/crates/kernel"),
            rust_version: None,
            dependencies: Vec::new(),
            targets: vec![
                target("/r/crates/kernel/src/lib.rs"),
                target("/r/crates/kernel/tests/t.rs"),
            ],
        }];
        let built = BTreeMap::from([(
            PathBuf::from("/r/crates/kernel/src/lib.rs"),
            BTreeSet::new(),
        )]);
        assert_eq!(unbuilt(&packages, &built), ["/r/crates/kernel/tests/t.rs"]);
    }
}
