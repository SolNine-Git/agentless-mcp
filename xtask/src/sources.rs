use std::fs::FileType;
use std::path::{Component, Path, PathBuf};

use crate::tokens::{self, Rule, Scan};
use crate::workspace::{self, Package};
use crate::{Failure, repo, tool_config};

const MANIFEST: &str = "Cargo.toml";
const SNAPSHOT: &str = "public-api.txt";
// The gates' own data: the third-party list, the erosion baseline, the layer table and the suppression ledger.
const GATE_DATA: [&str; 4] = [
    workspace::DEPENDENCIES,
    "crates/erosion-baseline.json",
    "crates/layers.toml",
    "crates/suppressions.toml",
];
// Data directories that build scripts, the generator and tests read, each named in review. A file
// that no rule allows fails, so a new tool's config file fails by default.
const DATA_DIRS: [&str; 20] = [
    "crates/app/fixtures",
    "crates/checks/fixtures",
    "crates/cli/fixtures",
    "crates/facts/fixtures",
    "crates/git/fixtures",
    "crates/graph/fixtures",
    "crates/hook/fixtures",
    "crates/io/fixtures",
    "crates/json/fixtures",
    "crates/mcp/fixtures",
    "crates/metrics/fixtures",
    "crates/patch/fixtures",
    "crates/patchcheck/fixtures",
    "crates/prompts/fixtures",
    "crates/render/fixtures",
    "crates/store/fixtures",
    "crates/syntax/fixtures",
    "crates/syntax/languages",
    "crates/syntax/vendor",
    "crates/text/fixtures",
];

/// One source file: its path relative to the root, and what its tokens hold.
pub struct Source {
    pub file: String,
    pub scan: Scan,
}

/// The `.rs` files under crates/ and xtask/, with the extension in any ASCII case, and each problem found while the scan read them.
pub struct Sources {
    pub files: Vec<Source>,
    pub problems: Vec<String>,
}

// Every entry of the walk below crates/ and xtask/ except directories, by kind.
#[derive(Default)]
struct Listing {
    sources: Vec<PathBuf>,
    data: Vec<PathBuf>,
    links: Vec<PathBuf>,
    special: Vec<PathBuf>,
}

/// Scan each `.rs` file under crates/ and xtask/ without following a symlink, and report each other entry outside the allowed set (git-ignored data files apart), misplaced packages and targets, test files that cargo does not build, other cases of `.rs`, and refused forms.
pub fn scan(root: &Path, packages: &[Package]) -> Result<Sources, Failure> {
    let listing = list_files(root)?;
    let ignored = ignored(root)?;
    let mut problems: Vec<String> = packages
        .iter()
        .flat_map(|package| package_problems(root, package))
        .collect();
    problems.extend(listing_problems(root, &listing, packages, &ignored));
    let files = listing
        .sources
        .iter()
        .map(|path| source(root, path))
        .collect::<Result<Vec<Source>, Failure>>()?;
    problems.extend(files.iter().flat_map(completeness_problems));
    Ok(Sources { files, problems })
}

fn list_files(root: &Path) -> Result<Listing, Failure> {
    let mut listing = Listing::default();
    let mut pending: Vec<PathBuf> = workspace::ROOTS.iter().map(|dir| root.join(dir)).collect();
    while let Some(dir) = pending.pop() {
        for (path, kind) in entries(&dir)? {
            if kind.is_symlink() {
                listing.links.push(path);
            } else if kind.is_dir() {
                pending.push(path);
            } else if !kind.is_file() {
                listing.special.push(path);
            } else if rs_in_any_case(&path) {
                listing.sources.push(path);
            } else {
                listing.data.push(path);
            }
        }
    }
    for paths in [
        &mut listing.sources,
        &mut listing.data,
        &mut listing.links,
        &mut listing.special,
    ] {
        paths.sort();
    }
    Ok(listing)
}

// Every git-ignored entry below crates/ and xtask/: a wholly ignored directory is one entry that ends in `/`.
fn ignored(root: &Path) -> Result<Vec<String>, Failure> {
    let mut args = vec![
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
        "--",
    ];
    args.extend(workspace::ROOTS);
    let output = repo::git(root, &args)?;
    let text = String::from_utf8(output)
        .map_err(|error| Failure::Broken(format!("git ls-files: {error}")))?;
    Ok(text
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect())
}

// No tool reads an ignored editor or system file. tool_config leaves crates/ and xtask/ to this walk, so an
// ignored tool config stays refused here.
fn ignored_data(file: &str, ignored: &[String]) -> bool {
    !tool_config::is_tool_config(file)
        && ignored.iter().any(|entry| {
            file == entry || (entry.ends_with('/') && file.starts_with(entry.as_str()))
        })
}

fn listing_problems(
    root: &Path,
    listing: &Listing,
    packages: &[Package],
    ignored: &[String],
) -> Vec<String> {
    let links = listing.links.iter().map(|link| {
        format!(
            "{}: a symlink under crates/ or xtask/ is refused: the gates do not follow it. Replace the link with a copy of its target",
            repo::relative(link, root)
        )
    });
    // An open of a FIFO blocks until a writer opens it, so the walk refuses the entry before any read.
    let special = listing.special.iter().map(|path| {
        format!(
            "{}: an entry that is not a regular file, a directory or a symlink is refused: a FIFO can block the scan. Remove it",
            repo::relative(path, root)
        )
    });
    let cases = listing.sources.iter().filter(|path| !rs_exactly(path)).map(|path| {
        format!(
            "{}: a .rs extension in another case is refused: a case-insensitive file system serves the file to rustc. Rename the extension to .rs",
            repo::relative(path, root)
        )
    });
    let data = listing
        .data
        .iter()
        .filter(|path| !ignored_data(&repo::relative(path, root), ignored))
        .filter_map(|path| data_problem(root, path, packages));
    let untested = packages
        .iter()
        .flat_map(|package| untested_problems(root, package, &listing.sources));
    links
        .chain(special)
        .chain(cases)
        .chain(data)
        .chain(untested)
        .collect()
}

fn data_problem(root: &Path, path: &Path, packages: &[Package]) -> Option<String> {
    let file = repo::relative(path, root);
    if tool_config::is_tool_config(&file) {
        return tool_config::stray_problem(&file);
    }
    let manifest = packages
        .iter()
        .any(|package| path == package.dir.join(MANIFEST));
    let snapshot = packages
        .iter()
        .any(|package| package.library().is_some() && path == package.dir.join(SNAPSHOT));
    let data = GATE_DATA.contains(&file.as_str())
        || DATA_DIRS
            .iter()
            .any(|dir| Path::new(&file).starts_with(dir));
    (!(manifest || snapshot || data)).then(|| {
        format!(
            "{file}: this file is refused: crates/ and xtask/ hold only package manifests, .rs files, a library's public-api.txt, the gate data, and the data directories that DATA_DIRS in xtask/src/sources.rs names. Move the file out, or name its data directory there in review"
        )
    })
}

fn untested_problems(root: &Path, package: &Package, sources: &[PathBuf]) -> Vec<String> {
    let tests = package.dir.join("tests");
    let built: Vec<&Path> = package
        .targets
        .iter()
        .filter(|target| target.kind.iter().any(|kind| kind == "test"))
        .map(|target| target.src_path.as_path())
        .collect();
    sources
        .iter()
        .filter(|path| discovered_test(path, &tests) && !built.contains(&path.as_path()))
        .map(|path| {
            format!(
                "{}: cargo builds no test target from this file, so nextest skips its tests. Remove autotests = false, or name the file in a [[test]] table",
                repo::relative(path, root)
            )
        })
        .collect()
}

// cargo finds a test target at tests/<name>.rs and at tests/<name>/main.rs.
fn discovered_test(path: &Path, tests: &Path) -> bool {
    let parent = path.parent();
    let main = path.file_name().is_some_and(|name| name == "main.rs");
    parent == Some(tests) || (main && parent.and_then(Path::parent) == Some(tests))
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
            "{name}: a workspace package must be xtask or live under crates/. Move the package under crates/"
        ));
    }
    problems.extend(
        package
            .targets
            .iter()
            .filter(|target| !inside(&target.src_path, &package.dir))
            .map(|target| {
                format!(
                    "{name}: target {} reads {}: a target's source must be a .rs file inside its package. Move the source into the package",
                    target.name,
                    repo::relative(&target.src_path, root)
                )
            }),
    );
    problems
}

fn inside(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir) && !climbs(path) && rs_exactly(path)
}

fn rs_exactly(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "rs")
}

fn rs_in_any_case(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
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
                "{file}:{}: #[path] is refused: rustc would read a file the gates do not scan. Put the module at its default path",
                meta.line
            )
        });
    let includes = scan
        .macro_calls
        .iter()
        .filter(|call| call.name == "include")
        .map(|call| {
            format!(
                "{file}:{}: include! is refused: rustc would read a file the gates do not scan. Write the code in a module",
                call.line
            )
        });
    let renames = scan
        .use_names
        .iter()
        .filter(|name| name.name == "include")
        .map(|name| {
            format!(
                "{file}:{}: a use item that names include is refused: it renames include!. Remove include from the use item",
                name.line
            )
        });
    let unreadable = scan.unreadable.iter().map(|found| {
        format!(
            "{file}:{}: the gates cannot read `{}`: an attribute fragment must come from a #[$name:meta] matcher",
            found.line, found.name
        )
    });
    let refused = scan
        .refused
        .iter()
        .map(|refused| format!("{file}:{}: {}", refused.line, reason(refused.rule)));
    paths
        .chain(includes)
        .chain(renames)
        .chain(unreadable)
        .chain(refused)
        .collect()
}

fn reason(rule: Rule) -> &'static str {
    match rule {
        Rule::Shebang => {
            "a first line that starts with #! and not #![ is refused: rustc skips it as a shebang. Delete the line"
        }
        Rule::LoneHash => {
            "a # that does not start #[..] or #![..] is refused: a macro could join it into an attribute. Write the whole attribute, or pass it as a #[$name:meta] fragment"
        }
        Rule::FragmentCall => {
            "a macro call named by a fragment is refused: the gates cannot see which macro it calls. Write the macro's name in the transcriber"
        }
        Rule::TtFragment => {
            "the tt fragment is refused: it can carry a lone # or a bracket group into an attribute. Use typed fragments, for example ($fmt:literal $(, $arg:expr)*)"
        }
        Rule::NestedDefinition => {
            "macro_rules! inside a macro_rules! body is refused: the gates cannot see the macro it defines. Define each macro at item level"
        }
        Rule::UseFragment => {
            "a fragment other than $crate in a use item is refused: the gates cannot see what it names. Write the path in the use item, or start it with $crate"
        }
        Rule::Cfg => {
            "a cfg predicate that names clippy or holds a fragment is refused: it can hide code from clippy. Write a literal predicate that does not name clippy"
        }
        Rule::IncludeName => {
            "the identifier include is refused: rustc reads a file through include!. Rename the item"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        Source, completeness_problems, data_problem, ignored_data, package_problems, rs_exactly,
        rs_in_any_case, scan, untested_problems,
    };
    use crate::repo::fixture::{git, scratch};
    use crate::tokens;
    use crate::workspace::{Package, Target};

    fn package(dir: &str, src_paths: &[&str]) -> Package {
        let targets = src_paths
            .iter()
            .map(|src_path| Target {
                name: "t".to_owned(),
                kind: vec!["lib".to_owned()],
                src_path: PathBuf::from(src_path),
                edition: "2024".to_owned(),
                test: true,
                required_features: None,
            })
            .collect();
        Package {
            name: "p".to_owned(),
            dir: PathBuf::from(dir),
            rust_version: None,
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
                "crates/kernel: target t reads crates/kernel/src/lib.txt: a target's source must be a .rs file inside its package. Move the source into the package",
                "crates/kernel: target t reads crates/kernel/../../docs/x.rs: a target's source must be a .rs file inside its package. Move the source into the package",
                "crates/kernel: target t reads docs/y.rs: a target's source must be a .rs file inside its package. Move the source into the package",
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

    #[test]
    fn a_refused_form_is_a_problem_with_its_line() {
        let source = Source {
            file: "crates/k/src/lib.rs".to_owned(),
            scan: tokens::scan("fn f() {}\nm!(#);\n").unwrap(),
        };
        assert_eq!(
            completeness_problems(&source),
            [
                "crates/k/src/lib.rs:2: a # that does not start #[..] or #![..] is refused: a macro could join it into an attribute. Write the whole attribute, or pass it as a #[$name:meta] fragment"
            ]
        );
    }

    #[test]
    fn a_file_outside_the_allowed_set_under_crates_and_xtask_is_refused() {
        let root = Path::new("/r");
        // package() gives a lib target per source path, so xtask here has no library.
        let packages = [
            package("/r/crates/kernel", &["/r/crates/kernel/src/lib.rs"]),
            package("/r/xtask", &[]),
        ];
        let problem = |file: &str| data_problem(root, &root.join(file), &packages);
        for allowed in [
            "crates/kernel/Cargo.toml",
            "crates/kernel/public-api.txt",
            "xtask/Cargo.toml",
            "xtask/clippy.toml",
            "crates/layers.toml",
            "crates/suppressions.toml",
            "crates/erosion-baseline.json",
        ] {
            assert_eq!(problem(allowed), None, "{allowed}");
        }
        for (refused, says) in [
            ("xtask/public-api.txt", "this file is refused"),
            ("crates/old/Cargo.toml", "this file is refused"),
            ("crates/kernel/README.md", "this file is refused"),
            ("crates/kernel/src/probe.r\u{17f}", "this file is refused"),
            ("crates/kernel/.ignore", "this tool config file is refused"),
            ("xtask/.clippy.toml", "this tool config file is refused"),
            (
                "crates/kernel/.agentless-mcp.json",
                "this tool config file is refused",
            ),
        ] {
            let found = problem(refused).unwrap_or_default();
            assert!(found.starts_with(&format!("{refused}: {says}")), "{found}");
        }
    }

    #[test]
    fn an_ignored_data_file_is_skipped_and_an_ignored_tool_config_is_not() {
        let ignored = [
            "crates/kernel/src/.lib.rs.swp".to_owned(),
            "crates/kernel/scratch/".to_owned(),
        ];
        for skipped in [
            "crates/kernel/src/.lib.rs.swp",
            "crates/kernel/scratch/notes.txt",
            "crates/kernel/scratch/deep/x.bin",
        ] {
            assert!(ignored_data(skipped, &ignored), "{skipped}");
        }
        for judged in [
            "crates/kernel/src/lib.rs.swp",
            "crates/kernel/scratchpad.txt",
            "crates/kernel/scratch/rustfmt.toml",
            "crates/kernel/scratch/.cargo/config.toml",
        ] {
            assert!(!ignored_data(judged, &ignored), "{judged}");
        }
    }

    #[test]
    fn the_scan_skips_the_data_files_that_git_ignores() {
        let dir = scratch("sources-ignored");
        git(&dir, &["init", "-q", "-b", "main"]);
        for sub in ["crates/kernel/src", "crates/kernel/scratch", "xtask"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join(".gitignore"), "*.swp\nscratch/\n").unwrap();
        for file in [
            "crates/kernel/src/.lib.rs.swp",
            "crates/kernel/scratch/notes.txt",
            "crates/kernel/notes.txt",
        ] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        let scanned = scan(&dir, &[]);
        std::fs::remove_dir_all(&dir).unwrap();
        let problems = scanned.unwrap().problems;
        let files: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split(':').next().unwrap_or_default())
            .collect();
        assert_eq!(files, ["crates/kernel/notes.txt"], "{problems:?}");
    }

    #[test]
    fn a_test_file_that_cargo_builds_no_target_from_is_refused() {
        let mut kernel = package("/r/crates/kernel", &["/r/crates/kernel/src/lib.rs"]);
        kernel.targets.push(Target {
            name: "tree".to_owned(),
            kind: vec!["test".to_owned()],
            src_path: PathBuf::from("/r/crates/kernel/tests/tree.rs"),
            edition: "2024".to_owned(),
            test: true,
            required_features: None,
        });
        let sources: Vec<PathBuf> = [
            "src/lib.rs",
            "tests/tree.rs",
            "tests/common/mod.rs",
            "tests/extra.rs",
            "tests/suite/main.rs",
        ]
        .iter()
        .map(|file| Path::new("/r/crates/kernel").join(file))
        .collect();
        let problems = untested_problems(Path::new("/r"), &kernel, &sources);
        let files: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split(':').next().unwrap_or_default())
            .collect();
        assert_eq!(
            files,
            [
                "crates/kernel/tests/extra.rs",
                "crates/kernel/tests/suite/main.rs"
            ]
        );
    }

    #[test]
    fn a_rs_extension_in_another_case_is_listed_but_is_not_exact() {
        for name in ["a.rs", "a.RS", "a.Rs"] {
            assert!(rs_in_any_case(Path::new(name)), "{name}");
        }
        assert!(rs_exactly(Path::new("a.rs")));
        assert!(!rs_exactly(Path::new("a.RS")));
        assert!(!rs_in_any_case(Path::new("a.rst")));
    }

    #[test]
    fn review_a_path_attribute_from_a_hash_fragment_is_refused() {
        let text = "macro_rules! h { ($h:tt) => { $h[path = \"p.txt\"] mod hidden; }; }\nh!(#);\n";
        let source = Source {
            file: "crates/k/src/lib.rs".to_owned(),
            scan: tokens::scan(text).unwrap(),
        };
        assert!(
            !completeness_problems(&source).is_empty(),
            "rustc reads p.txt here, and the gate reports nothing"
        );
    }
}
