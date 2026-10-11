use std::fs::FileType;
use std::path::{Component, Path, PathBuf};

use crate::tokens::{self, Scan};
use crate::workspace::Package;
use crate::{Failure, repo};

const ROOTS: [&str; 2] = ["crates", "xtask"];

/// One source file: its path relative to the root, and what its tokens hold.
pub struct Source {
    pub file: String,
    pub scan: Scan,
}

/// Every `.rs` file under crates/ and xtask/, and each way rustc could read a file outside that set.
pub struct Sources {
    pub files: Vec<Source>,
    pub problems: Vec<String>,
}

/// Scan every `.rs` file under crates/ and xtask/ without following a symlink; refuse what escapes the set.
pub fn scan(root: &Path, packages: &[Package]) -> Result<Sources, Failure> {
    let (paths, links) = walk(root)?;
    let mut problems: Vec<String> = packages
        .iter()
        .flat_map(|package| package_problems(root, package))
        .collect();
    problems.extend(links.iter().map(|link| {
        format!(
            "{}: a symlink under crates/ or xtask/ is refused",
            repo::relative(link, root)
        )
    }));
    let files = paths
        .iter()
        .map(|path| source(root, path))
        .collect::<Result<Vec<Source>, Failure>>()?;
    problems.extend(files.iter().flat_map(completeness_problems));
    Ok(Sources { files, problems })
}

fn walk(root: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>), Failure> {
    let mut files = Vec::new();
    let mut links = Vec::new();
    let mut pending: Vec<PathBuf> = ROOTS.iter().map(|dir| root.join(dir)).collect();
    while let Some(dir) = pending.pop() {
        for (path, kind) in entries(&dir)? {
            if kind.is_symlink() {
                links.push(path);
            } else if kind.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    links.sort();
    Ok((files, links))
}

fn entries(dir: &Path) -> Result<Vec<(PathBuf, FileType)>, Failure> {
    let read = std::fs::read_dir(dir).map_err(|error| repo::broken(dir, &error))?;
    read.map(|entry| {
        let entry = entry.map_err(|error| repo::broken(dir, &error))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| repo::broken(&path, &error))?;
        Ok((path, kind))
    })
    .collect()
}

fn source(root: &Path, path: &Path) -> Result<Source, Failure> {
    let file = repo::relative(path, root);
    let scan = tokens::scan(&repo::read(path)?)
        .map_err(|error| Failure::Broken(format!("{file}:{error}")))?;
    Ok(Source { file, scan })
}

fn package_problems(root: &Path, package: &Package) -> Vec<String> {
    let name = repo::relative(&package.dir, root);
    let placed = package.dir == root.join("xtask") || package.dir.starts_with(root.join("crates"));
    let mut problems = Vec::new();
    if !placed || climbs(&package.dir) {
        problems.push(format!(
            "{name}: a workspace package must be xtask or live under crates/"
        ));
    }
    problems.extend(
        package
            .targets
            .iter()
            .filter(|target| !inside(&target.src_path, &package.dir))
            .map(|target| {
                format!(
                    "{name}: target {} reads {}: a target's source must be a .rs file inside its package",
                    target.name,
                    repo::relative(&target.src_path, root)
                )
            }),
    );
    problems
}

fn inside(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir)
        && !climbs(path)
        && path.extension().is_some_and(|extension| extension == "rs")
}

fn climbs(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::ParentDir)
}

fn completeness_problems(source: &Source) -> Vec<String> {
    let (file, scan) = (&source.file, &source.scan);
    let paths = scan
        .attributes
        .iter()
        .filter(|meta| meta.path == "path")
        .map(|meta| {
            format!(
                "{file}:{}: #[path] is refused: rustc would read a file the gates do not scan",
                meta.line
            )
        });
    let includes = scan
        .macro_calls
        .iter()
        .filter(|call| call.name == "include")
        .map(|call| {
            format!(
                "{file}:{}: include! is refused: rustc would read a file the gates do not scan",
                call.line
            )
        });
    let renames = scan
        .use_names
        .iter()
        .filter(|name| name.name == "include")
        .map(|name| {
            format!(
                "{file}:{}: a use item that names include is refused: it renames include!",
                name.line
            )
        });
    let unreadable = scan.unreadable.iter().map(|found| {
        format!(
            "{file}:{}: the gates cannot read `{}`: an attribute fragment must come from a #[$name:meta] matcher",
            found.line, found.name
        )
    });
    paths
        .chain(includes)
        .chain(renames)
        .chain(unreadable)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Source, completeness_problems, package_problems};
    use crate::tokens;
    use crate::workspace::{Package, Target};

    fn package(dir: &str, src_paths: &[&str]) -> Package {
        let targets = src_paths
            .iter()
            .map(|src_path| Target {
                name: "t".to_owned(),
                kind: vec!["lib".to_owned()],
                src_path: PathBuf::from(src_path),
            })
            .collect();
        Package {
            name: "p".to_owned(),
            dir: PathBuf::from(dir),
            dependencies: Vec::new(),
            targets,
        }
    }

    #[test]
    fn a_target_outside_its_package_or_not_rs_is_refused() {
        let kernel = package(
            "/r/crates/kernel",
            &[
                "/r/crates/kernel/src/lib.rs",
                "/r/crates/kernel/build.rs",
                "/r/crates/kernel/src/lib.txt",
                "/r/crates/kernel/../../docs/x.rs",
                "/r/docs/y.rs",
            ],
        );
        let problems = package_problems(Path::new("/r"), &kernel);
        assert_eq!(
            problems,
            [
                "crates/kernel: target t reads crates/kernel/src/lib.txt: a target's source must be a .rs file inside its package",
                "crates/kernel: target t reads crates/kernel/../../docs/x.rs: a target's source must be a .rs file inside its package",
                "crates/kernel: target t reads docs/y.rs: a target's source must be a .rs file inside its package",
            ]
        );
    }

    #[test]
    fn a_package_outside_crates_and_xtask_is_refused() {
        let root = Path::new("/r");
        assert!(package_problems(root, &package("/r/xtask", &[])).is_empty());
        assert!(package_problems(root, &package("/r/crates/app/nested/foo", &[])).is_empty());
        for dir in ["/r/docs/evil", "/r/crates/../docs/evil", "/r"] {
            assert_eq!(package_problems(root, &package(dir, &[])).len(), 1, "{dir}");
        }
    }

    #[test]
    fn completeness_problems_name_path_include_use_and_fragments() {
        let text = "#[path = \"b.txt\"]\nmod b;\ninclude!(\"c.txt\");\nuse std::include as i;\n\
                    macro_rules! quiet { ($a:meta) => { #[$a] fn f() {} }; }\n\
                    const S: &str = include_str!(\"d.txt\");\n";
        let source = Source {
            file: "crates/k/src/lib.rs".to_owned(),
            scan: tokens::scan(text).unwrap(),
        };
        let problems = completeness_problems(&source);
        let lines: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split(':').nth(1).unwrap_or_default())
            .collect();
        assert_eq!(lines, ["1", "3", "4", "5"]);
    }
}
