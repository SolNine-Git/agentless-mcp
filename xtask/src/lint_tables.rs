use std::path::Path;

use toml::{Table, Value};

use crate::sources::{self, Source};
use crate::workspace::Package;
use crate::{Failure, repo, tool_config, verdict, workspace};

const SYNTAX: &str = "syntax";
const SYNTAX_FFI: &str = "crates/syntax/src/ffi.rs";
// The one crate that may write unsafe code adds these, and drops `unsafe_code`.
const SYNTAX_ADDS: [(&str, &str); 2] = [
    ("rust", "unsafe_op_in_unsafe_fn"),
    ("clippy", "undocumented_unsafe_blocks"),
];
// The crates under crates/ that may have a build script: a build script can set a cfg that only
// builds outside clippy see, so each one is listed here for review.
const BUILD_SCRIPTS: [&str; 1] = [SYNTAX];
// cargo builds a [patch] or [replace] package from wherever it points, outside every gate's scan.
const OVERRIDES: [&str; 2] = ["patch", "replace"];

/// Fail when a lint table or tool config drifts from the root, a manifest overrides a dependency, a build script is not listed, or `unsafe` leaves the syntax FFI file.
pub fn check(root: &Path) -> Result<(), Failure> {
    let manifest: Table = repo::toml(&root.join("Cargo.toml"))?;
    let lints = workspace_lints(&manifest)?;
    let root_clippy: Table = repo::toml(&root.join("clippy.toml"))?;
    let packages = workspace::packages(root)?;
    let mut problems = override_problems("Cargo.toml", &manifest);
    for package in &packages {
        let name = repo::relative(&package.dir, root);
        let own: Table = repo::toml(&package.dir.join("Cargo.toml"))?;
        problems.extend(lint_table_problem(&own, &name, &lints));
        problems.extend(override_problems(&format!("{name}/Cargo.toml"), &own));
        problems.extend(build_script_problem(&name, package));
    }
    problems.extend(tool_config::problems(root, &root_clippy, &packages)?);
    let sources = sources::scan(root, &packages)?;
    problems.extend(unsafe_problems(&sources.files));
    problems.extend(sources.problems);
    verdict(problems)
}

fn override_problems(file: &str, manifest: &Table) -> Vec<String> {
    OVERRIDES
        .into_iter()
        .filter(|table| manifest.contains_key(*table))
        .map(|table| {
            format!("{file}: [{table}] is refused: cargo would build a package that no gate reads")
        })
        .collect()
}

fn build_script_problem(name: &str, package: &Package) -> Option<String> {
    let listed: Vec<String> = BUILD_SCRIPTS
        .iter()
        .map(|crate_name| format!("crates/{crate_name}"))
        .collect();
    let script = package
        .targets
        .iter()
        .find(|target| target.kind.iter().any(|kind| kind == "custom-build"));
    script.filter(|_| !listed.iter().any(|dir| dir == name)).map(|target| {
        format!(
            "{name}: build script {} is refused: it can set a cfg that hides code from clippy, and only {} may have one",
            target.name,
            listed.join(", ")
        )
    })
}

fn workspace_lints(manifest: &Table) -> Result<Table, Failure> {
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(Value::as_table)
        .cloned()
        .ok_or_else(|| Failure::Broken("Cargo.toml has no [workspace.lints] table".to_owned()))
}

fn lint_table_problem(manifest: &Table, name: &str, lints: &Table) -> Option<String> {
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
    (found != Some(&expected)).then(|| {
        if syntax {
            format!("{name}/Cargo.toml: [lints] must be the workspace table without unsafe_code, plus the two unsafe lints at deny")
        } else {
            format!("{name}/Cargo.toml: [lints] must be `workspace = true` and nothing else")
        }
    })
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
    use std::path::PathBuf;

    use toml::Table;

    use super::{build_script_problem, override_problems, syntax_lints, unsafe_problems};
    use crate::sources::Source;
    use crate::tokens;
    use crate::workspace::{Package, Target};

    fn source(file: &str, text: &str) -> Source {
        Source {
            file: file.to_owned(),
            scan: tokens::scan(text).unwrap(),
        }
    }

    #[test]
    fn a_patch_or_replace_table_in_a_manifest_is_refused() {
        let manifest: Table =
            "[patch.crates-io]\nx = { path = \"vendor/x\" }\n[replace]\n\"y:1.0.0\" = { path = \"y\" }\n"
                .parse()
                .unwrap();
        assert_eq!(
            override_problems("Cargo.toml", &manifest),
            [
                "Cargo.toml: [patch] is refused: cargo would build a package that no gate reads",
                "Cargo.toml: [replace] is refused: cargo would build a package that no gate reads",
            ]
        );
        let plain: Table = "[dependencies]\npatch = \"1\"\n".parse().unwrap();
        assert!(override_problems("crates/k/Cargo.toml", &plain).is_empty());
    }

    #[test]
    fn a_build_script_outside_the_listed_crates_is_refused() {
        let package = |kinds: &[&str]| Package {
            name: "p".to_owned(),
            dir: PathBuf::from("/r/crates/p"),
            dependencies: Vec::new(),
            targets: kinds
                .iter()
                .map(|kind| Target {
                    name: format!("{kind}-target"),
                    kind: vec![(*kind).to_owned()],
                    src_path: PathBuf::from("/r/crates/p/build.rs"),
                })
                .collect(),
        };
        let scripted = package(&["lib", "custom-build"]);
        assert_eq!(
            build_script_problem("crates/kernel", &scripted),
            Some("crates/kernel: build script custom-build-target is refused: it can set a cfg that hides code from clippy, and only crates/syntax may have one".to_owned())
        );
        assert_eq!(build_script_problem("crates/syntax", &scripted), None);
        assert_eq!(
            build_script_problem("crates/kernel", &package(&["lib"])),
            None
        );
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
