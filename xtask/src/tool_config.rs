use std::path::Path;
use std::process::Command;

use toml::{Table, Value};

use crate::{Failure, process, repo};

const TOOL_FILES: [&str; 6] = [
    "clippy.toml",
    ".clippy.toml",
    "rustfmt.toml",
    ".rustfmt.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
];
const ALLOWED: [&str; 7] = [
    "clippy.toml",
    "rustfmt.toml",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "crates/io/clippy.toml",
    "crates/store/clippy.toml",
    "xtask/clippy.toml",
];
const CARGO_CONFIG: &str = ".cargo/config.toml";
const XTASK_ALIAS: &str = "run --quiet --package xtask --";
// The directories that may own a clippy.toml, and the disallowed-methods prefix each one drops.
const CLIPPY_EXEMPT: [(&str, &str); 3] = [
    ("crates/io", "std::fs::"),
    ("crates/store", "rusqlite::"),
    ("xtask", "std::fs::"),
];

/// Return each tool config file outside the allowed set, and each allowed one whose content drifts.
pub fn problems(root: &Path, root_clippy: &Table) -> Result<Vec<String>, Failure> {
    let mut problems = stray(&listed(root)?);
    problems.extend(cargo_config_problem(
        optional_table(root, CARGO_CONFIG)?.as_ref(),
    ));
    for (dir, prefix) in CLIPPY_EXEMPT {
        let file = format!("{dir}/clippy.toml");
        if let Some(found) = optional_table(root, &file)? {
            problems.extend(clippy_problem(&file, &found, root_clippy, prefix));
        }
    }
    Ok(problems)
}

fn listed(root: &Path) -> Result<Vec<String>, Failure> {
    let mut command = Command::new("git");
    command
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root);
    let output = process::stdout(&mut command, process::GIT)?;
    let text = String::from_utf8(output)
        .map_err(|error| Failure::Broken(format!("git ls-files: {error}")))?;
    let mut paths = Vec::new();
    for path in text.split('\0').filter(|path| !path.is_empty()) {
        if repo::exists(&root.join(path))? {
            paths.push(path.to_owned());
        }
    }
    Ok(paths)
}

fn optional_table(root: &Path, file: &str) -> Result<Option<Table>, Failure> {
    if repo::exists(&root.join(file))? {
        repo::toml(&root.join(file)).map(Some)
    } else {
        Ok(None)
    }
}

fn stray(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| is_tool_config(path) && !ALLOWED.contains(&path.as_str()))
        .map(|path| {
            format!("{path}: this tool config file is refused: clippy, rustfmt, rustup or cargo would read it")
        })
        .collect()
}

fn is_tool_config(path: &str) -> bool {
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

    use super::{cargo_config_problem, clippy_problem, stray, without_methods};

    fn strays(paths: &[&str]) -> Vec<String> {
        let paths: Vec<String> = paths.iter().map(|path| (*path).to_owned()).collect();
        stray(&paths)
            .into_iter()
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
