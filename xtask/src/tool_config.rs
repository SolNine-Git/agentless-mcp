use std::path::Path;

use toml::{Table, Value};

use crate::workspace::{self, Package};
use crate::{Failure, repo};

// cargo machete skips each path that an .ignore file names. .rgignore is the same format for the
// walkers that read it.
const TOOL_FILES: [&str; 13] = [
    "clippy.toml",
    ".clippy.toml",
    "rustfmt.toml",
    ".rustfmt.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
    "deny.toml",
    ".deny.toml",
    "deny.exceptions.toml",
    ".deny.exceptions.toml",
    ".agentless-mcp.json",
    ".ignore",
    ".rgignore",
];
// The root files that may exist. Each CLIPPY_EXEMPT directory may also hold a clippy.toml.
const ROOT_FILES: [&str; 6] = [
    ROOT_CLIPPY,
    "rustfmt.toml",
    "rust-toolchain.toml",
    CARGO_CONFIG,
    "deny.toml",
    ".agentless-mcp.json",
];
const ROOT_CLIPPY: &str = "clippy.toml";
const CARGO_CONFIG: &str = ".cargo/config.toml";
const XTASK_ALIAS: &str = "run --quiet --package xtask --";
// The directories that may own a clippy.toml, and the disallowed-methods prefix each one drops.
const CLIPPY_EXEMPT: [(&str, &str); 3] = [
    ("crates/io", "std::fs::"),
    ("crates/store", "rusqlite::"),
    ("xtask", "std::fs::"),
];
// clippy reads the first of these that it finds, from the package directory upward.
pub(crate) const CLIPPY_NAMES: [&str; 2] = [".clippy.toml", ROOT_CLIPPY];

/// Return each non-ASCII path, each tool config file outside crates/ and xtask/ and the allowed set, each allowed one whose content drifts, and each package that clippy configures from another file.
pub fn problems(
    root: &Path,
    root_clippy: &Table,
    packages: &[Package],
) -> Result<Vec<String>, Failure> {
    let listed = listed(root)?;
    let mut problems = non_ascii_problems(&listed);
    // The source walk judges every file under crates/ and xtask/ on disk, so it reports those.
    problems.extend(
        listed
            .iter()
            .filter(|path| {
                !workspace::ROOTS
                    .iter()
                    .any(|dir| Path::new(path).starts_with(dir))
            })
            .filter_map(|path| stray_problem(path)),
    );
    problems.extend(cargo_config_problem(
        optional_table(root, CARGO_CONFIG)?.as_ref(),
    ));
    for (dir, prefix) in CLIPPY_EXEMPT {
        let file = exempt_clippy(dir);
        if let Some(found) = optional_table(root, &file)? {
            problems.extend(clippy_problem(&file, &found, root_clippy, prefix));
        }
    }
    for package in packages {
        let dir = repo::relative(&package.dir, root);
        let nearest = nearest_clippy(root, &package.dir)?;
        problems.extend(scope_problem(&dir, nearest.as_deref()));
    }
    Ok(problems)
}

fn exempt_clippy(dir: &str) -> String {
    format!("{dir}/{ROOT_CLIPPY}")
}

fn nearest_clippy(root: &Path, dir: &Path) -> Result<Option<String>, Failure> {
    for above in dir.ancestors().take_while(|above| above.starts_with(root)) {
        for name in CLIPPY_NAMES {
            let file = above.join(name);
            if repo::exists(&file)? {
                return Ok(Some(repo::relative(&file, root)));
            }
        }
    }
    Ok(None)
}

fn scope_problem(dir: &str, nearest: Option<&str>) -> Option<String> {
    let exempt = CLIPPY_EXEMPT.iter().any(|&(exempt, _)| exempt == dir);
    let expected = if exempt {
        exempt_clippy(dir)
    } else {
        ROOT_CLIPPY.to_owned()
    };
    (nearest != Some(expected.as_str())).then(|| {
        format!(
            "{dir}: clippy reads {} for this package: it must read {expected}. Remove the nearer clippy config, or move the package",
            nearest.unwrap_or("no clippy config")
        )
    })
}

fn listed(root: &Path) -> Result<Vec<String>, Failure> {
    let visible = repo::git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    // An ignored file counts, because the tools read it. The listing matches names only, so it stays
    // small in target/ and .venv/, and a tool config name there fails too.
    let names = ignored_names();
    let mut args = vec![
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--",
    ];
    args.extend(names.iter().map(String::as_str));
    let ignored = repo::git(root, &args)?;
    let mut paths = Vec::new();
    for output in [visible, ignored] {
        let text = String::from_utf8(output)
            .map_err(|error| Failure::Broken(format!("git ls-files: {error}")))?;
        for path in text.split('\0').filter(|path| !path.is_empty()) {
            // --cached also lists a tracked file that the worktree deleted, and no tool reads it.
            if repo::exists(&root.join(path))? {
                paths.push(path.to_owned());
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn ignored_names() -> Vec<String> {
    TOOL_FILES
        .iter()
        .map(|name| format!(":(icase,glob)**/{name}"))
        .chain([":(icase,glob)**/.cargo/**".to_owned()])
        .collect()
}

fn non_ascii_problems(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !path.is_ascii())
        .map(|path| {
            format!(
                "{path}: a non-ASCII path is refused: APFS may fold its case beyond ASCII, and the gates fold ASCII case only. Rename it with ASCII characters"
            )
        })
        .collect()
}

fn optional_table(root: &Path, file: &str) -> Result<Option<Table>, Failure> {
    if repo::exists(&root.join(file))? {
        repo::toml(&root.join(file)).map(Some)
    } else {
        Ok(None)
    }
}

pub(crate) fn stray_problem(path: &str) -> Option<String> {
    (is_tool_config(path) && !allowed(path)).then(|| {
        format!(
            "{path}: this tool config file is refused: clippy, rustfmt, rustup, cargo, cargo-deny, cargo-machete, agentless-mcp or an ignore-file walker can read it. Delete it: put a setting in the root file, and write a test's config at run time"
        )
    })
}

fn allowed(path: &str) -> bool {
    ROOT_FILES.contains(&path)
        || CLIPPY_EXEMPT
            .iter()
            .any(|&(dir, _)| path == exempt_clippy(dir))
}

pub(crate) fn is_tool_config(path: &str) -> bool {
    // Compare in lowercase: a case-insensitive file system (macOS) serves Clippy.toml as clippy.toml.
    let lower = path.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    TOOL_FILES.contains(&name) || parts.contains(&".cargo")
}

fn cargo_config_problem(found: Option<&Table>) -> Option<String> {
    let alias = Table::from_iter([("xtask".to_owned(), Value::String(XTASK_ALIAS.to_owned()))]);
    let expected = Table::from_iter([("alias".to_owned(), Value::Table(alias))]);
    (found != Some(&expected)).then(|| {
        format!("{CARGO_CONFIG} must hold only the xtask alias: [alias] xtask = \"{XTASK_ALIAS}\"")
    })
}

fn clippy_problem(file: &str, found: &Table, root_clippy: &Table, prefix: &str) -> Option<String> {
    (*found != without_methods(root_clippy, prefix)).then(|| {
        format!("{file} must equal the root clippy.toml without the {prefix} disallowed methods")
    })
}

fn without_methods(root_clippy: &Table, prefix: &str) -> Table {
    let mut table = root_clippy.clone();
    if let Some(Value::Array(methods)) = table.get_mut("disallowed-methods") {
        methods.retain(|method| {
            let path = method.get("path").and_then(Value::as_str);
            path.is_none_or(|path| !path.starts_with(prefix))
        });
    }
    table
}

#[cfg(test)]
mod tests {
    use toml::Table;

    use super::{
        cargo_config_problem, clippy_problem, ignored_names, non_ascii_problems, scope_problem,
        stray_problem, without_methods,
    };

    fn strays(paths: &[&str]) -> Vec<String> {
        paths
            .iter()
            .filter_map(|path| stray_problem(path))
            .map(|problem| problem.split(':').next().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn a_tool_config_file_outside_the_allowed_set_is_refused() {
        let refused = [
            "crates/clippy.toml",
            "crates/kernel/.clippy.toml",
            ".clippy.toml",
            "crates/kernel/.cargo/config.toml",
            ".cargo/config",
            "crates/kernel/rustfmt.toml",
            ".rustfmt.toml",
            "rust-toolchain",
            "crates/kernel/rust-toolchain.toml",
            "crates/Clippy.toml",
            ".ignore",
            "crates/.IGNORE",
            ".rgignore",
        ];
        assert_eq!(strays(&refused), refused);
        let allowed = [
            "clippy.toml",
            "rustfmt.toml",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "crates/io/clippy.toml",
            "crates/store/clippy.toml",
            "xtask/clippy.toml",
            "docs/clippy.md",
            "crates/kernel/src/cargo.rs",
        ];
        assert!(strays(&allowed).is_empty());
    }

    #[test]
    fn a_deny_or_agentless_config_outside_the_root_deny_toml_and_json_is_refused() {
        let refused = [
            "deny.exceptions.toml",
            ".deny.exceptions.toml",
            ".deny.toml",
            "Deny.Exceptions.TOML",
            "crates/deny.toml",
            "crates/.agentless-mcp.json",
            "tests/fixtures/.Agentless-MCP.json",
        ];
        assert_eq!(strays(&refused), refused);
        assert!(strays(&["deny.toml", ".agentless-mcp.json", "docs/deny.md"]).is_empty());
    }

    #[test]
    fn a_package_reads_the_root_clippy_config_and_an_exempt_one_reads_its_own() {
        assert_eq!(scope_problem("crates/kernel", Some("clippy.toml")), None);
        assert_eq!(
            scope_problem("crates/io", Some("crates/io/clippy.toml")),
            None
        );
        assert_eq!(scope_problem("xtask", Some("xtask/clippy.toml")), None);
        for (dir, nearest) in [
            ("crates/io/evil", Some("crates/io/clippy.toml")),
            ("crates/kernel", Some(".clippy.toml")),
            ("crates/kernel", None),
            ("xtask", Some("xtask/.clippy.toml")),
            ("xtask", Some("clippy.toml")),
        ] {
            assert!(scope_problem(dir, nearest).is_some(), "{dir} {nearest:?}");
        }
        assert_eq!(
            scope_problem("xtask", Some("xtask/.clippy.toml")).unwrap(),
            "xtask: clippy reads xtask/.clippy.toml for this package: it must read xtask/clippy.toml. Remove the nearer clippy config, or move the package"
        );
    }

    #[test]
    fn review_a_root_config_in_another_case_is_refused() {
        let paths = [
            "Clippy.toml",
            ".Cargo/config.toml",
            "RUSTFMT.TOML",
            "Rust-Toolchain",
        ];
        assert_eq!(strays(&paths), paths);
    }

    #[test]
    fn a_non_ascii_path_is_refused() {
        let paths: Vec<String> = [
            "ru\u{17f}tfmt.toml",
            "crates/kernel/src/probe.r\u{17f}",
            "docs/a.md",
        ]
        .iter()
        .map(|path| (*path).to_owned())
        .collect();
        let problems = non_ascii_problems(&paths);
        assert_eq!(problems.len(), 2);
        for (problem, path) in problems.iter().zip(&paths) {
            assert!(
                problem.starts_with(&format!("{path}: a non-ASCII path is refused")),
                "{problem}"
            );
        }
    }

    #[test]
    fn the_ignored_listing_names_every_tool_config_in_any_case() {
        let names = ignored_names();
        assert!(names.contains(&":(icase,glob)**/.clippy.toml".to_owned()));
        assert!(names.contains(&":(icase,glob)**/.ignore".to_owned()));
        assert_eq!(names.last().unwrap(), ":(icase,glob)**/.cargo/**");
    }

    #[test]
    fn the_root_cargo_config_holds_only_the_xtask_alias() {
        let parse = |text: &str| text.parse::<Table>().unwrap();
        let exact = parse("[alias]\nxtask = \"run --quiet --package xtask --\"\n");
        assert_eq!(cargo_config_problem(Some(&exact)), None);
        let flags = parse(
            "[alias]\nxtask = \"run --quiet --package xtask --\"\n[build]\nrustflags = [\"--cap-lints\", \"allow\"]\n",
        );
        let shadow =
            parse("[alias]\nxtask = \"run --quiet --package xtask --\"\nmachete = \"version\"\n");
        for found in [Some(&flags), Some(&shadow), None] {
            assert!(cargo_config_problem(found).is_some());
        }
    }

    #[test]
    fn an_exempt_directory_drops_only_its_own_prefix() {
        let root: Table = "disallowed-methods = [{ path = \"std::fs::write\" }, { path = \"rusqlite::Connection::open\" }]"
            .parse()
            .unwrap();
        let expected: Table = "disallowed-methods = [{ path = \"rusqlite::Connection::open\" }]"
            .parse()
            .unwrap();
        assert_eq!(without_methods(&root, "std::fs::"), expected);
        assert_eq!(
            clippy_problem("xtask/clippy.toml", &expected, &root, "std::fs::"),
            None
        );
        assert!(
            clippy_problem("crates/store/clippy.toml", &expected, &root, "rusqlite::").is_some()
        );
    }
}
