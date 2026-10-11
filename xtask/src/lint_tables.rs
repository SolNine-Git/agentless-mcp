use std::path::Path;

use toml::{Table, Value};

use crate::{Failure, repo, verdict};

const SYNTAX: &str = "syntax";
const SYNTAX_FFI: &str = "ffi.rs";
// The one crate that may write unsafe code adds these, and drops `unsafe_code`.
const SYNTAX_ADDS: [(&str, &str); 2] = [
    ("rust", "unsafe_op_in_unsafe_fn"),
    ("clippy", "undocumented_unsafe_blocks"),
];
// The directories that may own a clippy.toml, and the disallowed-methods prefix each one drops.
const CLIPPY_EXEMPT: [(&str, &str); 3] = [
    ("crates/io", "std::fs::"),
    ("crates/store", "rusqlite::"),
    ("xtask", "std::fs::"),
];

/// Fail when a crate's lint table or clippy.toml drifts from the root, or `unsafe` leaves `syntax`'s FFI file.
pub fn check(root: &Path) -> Result<(), Failure> {
    let manifest: Table = repo::toml(&root.join("Cargo.toml"))?;
    let lints = workspace_lints(&manifest)?;
    let root_clippy: Table = repo::toml(&root.join("clippy.toml"))?;
    let mut dirs = repo::crate_dirs(root)?;
    dirs.push(root.join("xtask"));
    let mut problems = Vec::new();
    for dir in &dirs {
        let name = repo::relative(dir, root);
        problems.extend(lint_table_problem(dir, &name, &lints)?);
        problems.extend(clippy_problem(dir, &name, &root_clippy)?);
    }
    problems.extend(unsafe_problems(root)?);
    verdict(problems)
}

fn workspace_lints(manifest: &Table) -> Result<Table, Failure> {
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(Value::as_table)
        .cloned()
        .ok_or_else(|| Failure::Broken("Cargo.toml has no [workspace.lints] table".to_owned()))
}

fn lint_table_problem(dir: &Path, name: &str, lints: &Table) -> Result<Option<String>, Failure> {
    let manifest: Table = repo::toml(&dir.join("Cargo.toml"))?;
    let found = manifest.get("lints");
    let syntax = name == format!("crates/{SYNTAX}");
    let expected = if syntax {
        Value::Table(syntax_lints(lints))
    } else {
        Value::Table(Table::from_iter([(
            "workspace".to_owned(),
            Value::Boolean(true),
        )]))
    };
    Ok((found != Some(&expected)).then(|| {
        if syntax {
            format!("{name}/Cargo.toml: [lints] must be the workspace table without unsafe_code, plus the two unsafe lints at deny")
        } else {
            format!("{name}/Cargo.toml: [lints] must be `workspace = true` and nothing else")
        }
    }))
}

fn syntax_lints(lints: &Table) -> Table {
    let mut table = lints.clone();
    if let Some(Value::Table(rust)) = table.get_mut("rust") {
        rust.remove("unsafe_code");
    }
    for (group, lint) in SYNTAX_ADDS {
        let entry = table
            .entry(group)
            .or_insert_with(|| Value::Table(Table::new()));
        if let Value::Table(group) = entry {
            group.insert(lint.to_owned(), Value::String("deny".to_owned()));
        }
    }
    table
}

fn clippy_problem(dir: &Path, name: &str, root_clippy: &Table) -> Result<Option<String>, Failure> {
    let path = dir.join("clippy.toml");
    if !path.is_file() {
        return Ok(None);
    }
    let found: Table = repo::toml(&path)?;
    let dropped = CLIPPY_EXEMPT
        .iter()
        .find(|(exempt, _)| *exempt == name)
        .map(|&(_, prefix)| prefix);
    let expected = without_methods(root_clippy, dropped);
    Ok((found != expected).then(|| match dropped {
        Some(prefix) => format!("{name}/clippy.toml must equal the root clippy.toml without the {prefix} disallowed methods"),
        None => format!("{name}/clippy.toml must equal the root clippy.toml"),
    }))
}

fn without_methods(root_clippy: &Table, prefix: Option<&str>) -> Table {
    let mut table = root_clippy.clone();
    if let (Some(prefix), Some(Value::Array(methods))) =
        (prefix, table.get_mut("disallowed-methods"))
    {
        methods.retain(|method| {
            let path = method.get("path").and_then(Value::as_str);
            path.is_none_or(|path| !path.starts_with(prefix))
        });
    }
    table
}

fn unsafe_problems(root: &Path) -> Result<Vec<String>, Failure> {
    let source = root.join("crates").join(SYNTAX).join("src");
    if !source.is_dir() {
        return Ok(Vec::new());
    }
    let mut problems = Vec::new();
    for path in repo::rust_files(&source)? {
        if path.file_name().is_some_and(|file| file == SYNTAX_FFI) {
            continue;
        }
        if mentions_unsafe(&repo::read(&path)?) {
            let file = repo::relative(&path, root);
            problems.push(format!(
                "{file}: unsafe code belongs in crates/{SYNTAX}/src/{SYNTAX_FFI} only"
            ));
        }
    }
    Ok(problems)
}

fn mentions_unsafe(text: &str) -> bool {
    text.match_indices("unsafe").any(|(index, word)| {
        let before = text[..index].chars().next_back();
        let after = text[index + word.len()..].chars().next();
        let is_ident = |character: char| character.is_alphanumeric() || character == '_';
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

#[cfg(test)]
mod tests {
    use super::{mentions_unsafe, syntax_lints, without_methods};
    use toml::Table;

    #[test]
    fn the_syntax_table_drops_unsafe_code_and_adds_the_unsafe_lints() {
        let lints: Table = "rust = { unsafe_code = \"forbid\" }\nclippy = { pedantic = \"warn\" }"
            .parse()
            .unwrap();
        let expected: Table = "rust = { unsafe_op_in_unsafe_fn = \"deny\" }\n\
                               clippy = { pedantic = \"warn\", undocumented_unsafe_blocks = \"deny\" }"
            .parse()
            .unwrap();
        assert_eq!(syntax_lints(&lints), expected);
    }

    #[test]
    fn an_exempt_directory_drops_only_its_own_prefix() {
        let root: Table = "disallowed-methods = [{ path = \"std::fs::write\" }, { path = \"rusqlite::Connection::open\" }]"
            .parse()
            .unwrap();
        let expected: Table = "disallowed-methods = [{ path = \"rusqlite::Connection::open\" }]"
            .parse()
            .unwrap();
        assert_eq!(without_methods(&root, Some("std::fs::")), expected);
        assert_eq!(without_methods(&root, None), root);
    }

    #[test]
    fn unsafe_is_found_as_a_word_only() {
        assert!(mentions_unsafe("pub unsafe fn f() {}"));
        assert!(mentions_unsafe("let x = unsafe { y };"));
        assert!(!mentions_unsafe("forbid(unsafe_code) and is_unsafe()"));
    }
}
