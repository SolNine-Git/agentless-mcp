use std::path::Path;

use toml::{Table, Value};

use crate::sources::{self, Source};
use crate::{Failure, repo, tool_config, verdict, workspace};

const SYNTAX: &str = "syntax";
const SYNTAX_FFI: &str = "crates/syntax/src/ffi.rs";
// The one crate that may write unsafe code adds these, and drops `unsafe_code`.
const SYNTAX_ADDS: [(&str, &str); 2] = [
    ("rust", "unsafe_op_in_unsafe_fn"),
    ("clippy", "undocumented_unsafe_blocks"),
];

/// Fail when a lint table or tool config file drifts from the root, or `unsafe` leaves the syntax FFI file.
pub fn check(root: &Path) -> Result<(), Failure> {
    let manifest: Table = repo::toml(&root.join("Cargo.toml"))?;
    let lints = workspace_lints(&manifest)?;
    let root_clippy: Table = repo::toml(&root.join("clippy.toml"))?;
    let packages = workspace::packages(root)?;
    let mut problems = Vec::new();
    for package in &packages {
        let name = repo::relative(&package.dir, root);
        problems.extend(lint_table_problem(&package.dir, &name, &lints)?);
    }
    problems.extend(tool_config::problems(root, &root_clippy)?);
    let sources = sources::scan(root, &packages)?;
    problems.extend(unsafe_problems(&sources.files));
    problems.extend(sources.problems);
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

fn unsafe_problems(files: &[Source]) -> Vec<String> {
    let mut problems = Vec::new();
    for source in files {
        if source.file == SYNTAX_FFI {
            problems.extend(source.scan.macro_definitions.iter().map(|definition| {
                format!(
                    "{SYNTAX_FFI}:{}: macro_rules! is refused here: a macro would carry unsafe out of the file",
                    definition.line
                )
            }));
        } else {
            problems.extend(
                source
                    .scan
                    .words
                    .iter()
                    .filter(|word| word.name == "unsafe")
                    .map(|word| {
                        format!(
                            "{}:{}: unsafe code belongs in {SYNTAX_FFI} only",
                            source.file, word.line
                        )
                    }),
            );
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use toml::Table;

    use super::{syntax_lints, unsafe_problems};
    use crate::sources::Source;
    use crate::tokens;

    fn source(file: &str, text: &str) -> Source {
        Source {
            file: file.to_owned(),
            scan: tokens::scan(text).unwrap(),
        }
    }

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
    fn unsafe_outside_the_ffi_file_is_a_problem_with_its_line() {
        let text = "fn f() {\n    unsafe {}\n}\n";
        let files = [
            source("crates/syntax/build.rs", text),
            source("crates/syntax/tests/t.rs", text),
            source("crates/syntax/src/sub/ffi.rs", text),
            source("crates/syntax/src/ffi.rs", text),
        ];
        assert_eq!(
            unsafe_problems(&files),
            [
                "crates/syntax/build.rs:2: unsafe code belongs in crates/syntax/src/ffi.rs only",
                "crates/syntax/tests/t.rs:2: unsafe code belongs in crates/syntax/src/ffi.rs only",
                "crates/syntax/src/sub/ffi.rs:2: unsafe code belongs in crates/syntax/src/ffi.rs only",
            ]
        );
    }

    #[test]
    fn a_macro_in_the_ffi_file_is_a_problem() {
        let files = [source(
            "crates/syntax/src/ffi.rs",
            "\nmacro_rules! raw {\n    () => { unsafe {} };\n}\n",
        )];
        assert_eq!(
            unsafe_problems(&files),
            [
                "crates/syntax/src/ffi.rs:2: macro_rules! is refused here: a macro would carry unsafe out of the file"
            ]
        );
    }
}
