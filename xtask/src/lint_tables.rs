use std::path::Path;

use toml::{Table, Value};

use crate::sources::{self, Source};
use crate::workspace::{BUILD_SCRIPT, Package, Target};
use crate::{Failure, repo, tool_config, verdict, workspace};

const SYNTAX: &str = "syntax";
const SYNTAX_FFI: &str = "crates/syntax/src/ffi.rs";
// The one crate that may write unsafe code adds these, and drops `unsafe_code`.
const SYNTAX_ADDS: [(&str, &str); 2] = [
    ("rust", "unsafe_op_in_unsafe_fn"),
    ("clippy", "undocumented_unsafe_blocks"),
];
// rustc's unsafe_code lint covers global_asm!, which holds no `unsafe` token in edition 2024.
const UNSAFE_WORDS: [&str; 2] = ["unsafe", "global_asm"];
// The crates under crates/ that may have a build script: a build script can set a cfg that only
// builds outside clippy see, so each one is listed here for review.
const BUILD_SCRIPTS: [&str; 1] = [SYNTAX];
// In the root manifest, cargo builds a [patch] or [replace] package from wherever it points, outside
// every gate's scan. cargo ignores both tables in a package manifest.
const OVERRIDES: [&str; 2] = ["patch", "replace"];
const TARGET_TABLES: [&str; 5] = ["lib", "bin", "test", "bench", "example"];

// The settings that every package inherits from [workspace.package].
struct Inherited {
    edition: String,
    rust_version: String,
}

/// Fail when a lint table or tool config drifts from the root, a manifest overrides a dependency or turns tests off, a target leaves the workspace edition or rust-version, a build script is not listed, or unsafe code leaves the syntax FFI file.
pub fn check(root: &Path) -> Result<(), Failure> {
    let manifest: Table = repo::toml(&root.join("Cargo.toml"))?;
    let lints = workspace_lints(&manifest)?;
    let inherited = inherited(&manifest)?;
    let root_clippy: Table = repo::toml(&root.join("clippy.toml"))?;
    let packages = workspace::packages(root)?;
    let mut problems = override_problems("Cargo.toml", &manifest);
    for package in &packages {
        let name = repo::relative(&package.dir, root);
        let own: Table = repo::toml(&package.dir.join("Cargo.toml"))?;
        problems.extend(manifest_problems(&name, &own, &lints));
        problems.extend(target_problems(&name, package, &inherited));
    }
    problems.extend(tool_config::problems(root, &root_clippy, &packages)?);
    let sources = sources::scan(root, &packages)?;
    problems.extend(unsafe_problems(&sources.files));
    problems.extend(sources.problems);
    verdict(problems)
}

fn inherited(manifest: &Table) -> Result<Inherited, Failure> {
    Ok(Inherited {
        edition: workspace::package_key(manifest, "edition")?,
        rust_version: workspace::package_key(manifest, "rust-version")?,
    })
}

fn manifest_problems(name: &str, own: &Table, lints: &Table) -> Vec<String> {
    let file = format!("{name}/Cargo.toml");
    let mut problems: Vec<String> = lint_table_problem(own, name, lints).into_iter().collect();
    problems.extend(override_problems(&file, own));
    problems.extend(package_table_problems(&file, own));
    problems.extend(harness_problems(&file, own));
    problems
}

fn target_problems(name: &str, package: &Package, inherited: &Inherited) -> Vec<String> {
    let mut problems: Vec<String> = build_script_problem(name, package).into_iter().collect();
    problems.extend(rust_version_problem(name, package, &inherited.rust_version));
    for target in &package.targets {
        problems.extend(edition_problem(name, target, &inherited.edition));
        problems.extend(test_problems(name, target));
    }
    problems
}

fn override_problems(file: &str, manifest: &Table) -> Vec<String> {
    OVERRIDES
        .into_iter()
        .filter(|table| manifest.contains_key(*table))
        .map(|table| {
            format!(
                "{file}: [{table}] is refused: in the root manifest cargo builds a package that no gate reads, and elsewhere cargo ignores it. Depend on a published version, or make the crate a workspace member under crates/"
            )
        })
        .collect()
}

fn package_table_problems(file: &str, manifest: &Table) -> Vec<String> {
    let mut problems = Vec::new();
    if manifest.contains_key("profile") {
        problems.push(format!(
            "{file}: [profile] is refused: cargo ignores a profile outside the root manifest. Set the profile in the root Cargo.toml, with a Policy-Change trailer"
        ));
    }
    let machete = manifest
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("cargo-machete"));
    if machete.is_some() {
        problems.push(format!(
            "{file}: [package.metadata.cargo-machete] is refused: it hides a dependency from cargo machete. Remove the unused dependency"
        ));
    }
    problems
}

// cargo metadata does not report `harness`, so the manifest is the only source for it.
fn harness_problems(file: &str, manifest: &Table) -> Vec<String> {
    TARGET_TABLES
        .into_iter()
        .flat_map(|key| {
            target_tables(manifest, key)
                .into_iter()
                .filter(|table| table.get("harness") == Some(&Value::Boolean(false)))
                .map(move |_| {
                    format!(
                        "{file}: harness = false in a {key} target is refused: nextest then runs the target's own main, which can report no test. Remove the key, and write #[test] functions"
                    )
                })
        })
        .collect()
}

fn target_tables<'a>(manifest: &'a Table, key: &str) -> Vec<&'a Table> {
    match manifest.get(key) {
        Some(Value::Table(table)) => vec![table],
        Some(Value::Array(tables)) => tables.iter().filter_map(Value::as_table).collect(),
        _ => Vec::new(),
    }
}

fn rust_version_problem(name: &str, package: &Package, rust_version: &str) -> Option<String> {
    let found = package.rust_version.as_deref();
    (found != Some(rust_version)).then(|| {
        format!(
            "{name}: rust-version is {}, not the workspace rust-version {rust_version}: clippy turns off each lint whose fix needs a newer Rust. Write rust-version.workspace = true",
            found.unwrap_or("unset")
        )
    })
}

fn edition_problem(name: &str, target: &Target, edition: &str) -> Option<String> {
    (target.edition != edition).then(|| {
        format!(
            "{name}: target {} builds with edition {}, not the workspace edition {edition}: an older edition accepts an unsafe attribute with no unsafe token. Write edition.workspace = true, and remove edition from each target table",
            target.name, target.edition
        )
    })
}

fn test_problems(name: &str, target: &Target) -> Vec<String> {
    let mut problems = Vec::new();
    if target.tested_by_default() && !target.test {
        problems.push(format!(
            "{name}: target {} sets test = false: cargo test and nextest then skip its tests. Remove the key",
            target.name
        ));
    }
    let required = target.required_features.as_ref();
    if required.is_some_and(|features| !features.is_empty()) {
        problems.push(format!(
            "{name}: target {} has required-features: cargo skips the target, its lints and its tests unless those features are on. Remove required-features",
            target.name
        ));
    }
    problems
}

fn build_script_problem(name: &str, package: &Package) -> Option<String> {
    let listed: Vec<String> = BUILD_SCRIPTS
        .iter()
        .map(|crate_name| format!("crates/{crate_name}"))
        .collect();
    let script = package
        .targets
        .iter()
        .find(|target| target.kind.iter().any(|kind| kind == BUILD_SCRIPT));
    script.filter(|_| !listed.iter().any(|dir| dir == name)).map(|target| {
        format!(
            "{name}: build script {} is refused: it can set a cfg that hides code from clippy, and only {} may have one. Move the logic into the crate's code",
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

/// Return the lints that the syntax package raises above the workspace levels, as rustc names them.
pub fn syntax_raised() -> Vec<String> {
    SYNTAX_ADDS
        .iter()
        .map(|(group, lint)| {
            if *group == "clippy" {
                format!("clippy::{lint}")
            } else {
                (*lint).to_owned()
            }
        })
        .collect()
}

fn unsafe_problems(files: &[Source]) -> Vec<String> {
    let mut problems = Vec::new();
    for source in files {
        if source.file == SYNTAX_FFI {
            problems.extend(source.scan.macro_definitions.iter().map(|definition| {
                format!(
                    "{SYNTAX_FFI}:{}: macro_rules! is refused here: a macro would carry unsafe out of the file. Write each unsafe item out in full",
                    definition.line
                )
            }));
        } else {
            problems.extend(
                source
                    .scan
                    .words
                    .iter()
                    .filter(|word| UNSAFE_WORDS.contains(&word.name.as_str()))
                    .map(|word| {
                        format!(
                            "{}:{}: `{}` is unsafe code, and unsafe code belongs in {SYNTAX_FFI} only. Move it into that file",
                            source.file, word.line, word.name
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

    use super::{
        Inherited, build_script_problem, inherited, manifest_problems, override_problems,
        syntax_lints, target_problems, unsafe_problems,
    };
    use crate::Failure;
    use crate::sources::Source;
    use crate::tokens;
    use crate::workspace::{Package, Target};

    fn source(file: &str, text: &str) -> Source {
        Source {
            file: file.to_owned(),
            scan: tokens::scan(text).unwrap(),
        }
    }

    fn target(kind: &str) -> Target {
        Target {
            name: format!("{kind}-target"),
            kind: vec![kind.to_owned()],
            src_path: PathBuf::from("/r/crates/p/build.rs"),
            edition: "2024".to_owned(),
            test: true,
            required_features: None,
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
                "Cargo.toml: [patch] is refused: in the root manifest cargo builds a package that no gate reads, and elsewhere cargo ignores it. Depend on a published version, or make the crate a workspace member under crates/",
                "Cargo.toml: [replace] is refused: in the root manifest cargo builds a package that no gate reads, and elsewhere cargo ignores it. Depend on a published version, or make the crate a workspace member under crates/",
            ]
        );
        let plain: Table = "[dependencies]\npatch = \"1\"\n".parse().unwrap();
        assert!(override_problems("crates/k/Cargo.toml", &plain).is_empty());
    }

    #[test]
    fn a_package_profile_machete_list_or_harness_false_is_refused() {
        let lints: Table = "rust = { unsafe_code = \"forbid\" }".parse().unwrap();
        let manifest: Table =
            "[package]\nname = \"k\"\n[package.metadata.cargo-machete]\nignored = [\"itoa\"]\n\
                               [lints]\nworkspace = true\n[profile.dev]\ndebug-assertions = false\n\
                               [lib]\nharness = false\n[[test]]\nname = \"a\"\nharness = true\n\
                               [[test]]\nname = \"b\"\nharness = false\n"
                .parse()
                .unwrap();
        let problems = manifest_problems("crates/k", &manifest, &lints);
        let heads: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split(" is refused").next().unwrap_or_default())
            .collect();
        assert_eq!(
            heads,
            [
                "crates/k/Cargo.toml: [profile]",
                "crates/k/Cargo.toml: [package.metadata.cargo-machete]",
                "crates/k/Cargo.toml: harness = false in a lib target",
                "crates/k/Cargo.toml: harness = false in a test target",
            ]
        );
        let plain: Table =
            "[package]\nname = \"k\"\n[package.metadata.docs]\nx = 1\n[lints]\nworkspace = true\n"
                .parse()
                .unwrap();
        assert!(manifest_problems("crates/k", &plain, &lints).is_empty());
    }

    #[test]
    fn a_target_off_the_workspace_edition_or_a_package_off_its_rust_version_is_refused() {
        let inherited = Inherited {
            edition: "2024".to_owned(),
            rust_version: "1.94".to_owned(),
        };
        let mut old = target("lib");
        old.edition = "2021".to_owned();
        let mut package = Package {
            name: "p".to_owned(),
            dir: PathBuf::from("/r/crates/p"),
            rust_version: Some("1.94".to_owned()),
            dependencies: Vec::new(),
            targets: vec![target("lib"), old],
        };
        assert_eq!(
            target_problems("crates/p", &package, &inherited),
            [
                "crates/p: target lib-target builds with edition 2021, not the workspace edition 2024: an older edition accepts an unsafe attribute with no unsafe token. Write edition.workspace = true, and remove edition from each target table"
            ]
        );
        package.targets.pop();
        assert!(target_problems("crates/p", &package, &inherited).is_empty());
        for found in [Some("1.85"), None] {
            package.rust_version = found.map(str::to_owned);
            assert_eq!(
                target_problems("crates/p", &package, &inherited).len(),
                1,
                "{found:?}"
            );
        }
    }

    #[test]
    fn a_target_whose_tests_cargo_skips_is_refused() {
        let inherited = Inherited {
            edition: "2024".to_owned(),
            rust_version: "1.94".to_owned(),
        };
        let untested = |kind: &str| Target {
            test: false,
            ..target(kind)
        };
        let gated = |features: &[&str]| Target {
            required_features: Some(
                features
                    .iter()
                    .map(|feature| (*feature).to_owned())
                    .collect(),
            ),
            ..target("test")
        };
        let package = |targets: Vec<Target>| Package {
            name: "p".to_owned(),
            dir: PathBuf::from("/r/crates/p"),
            rust_version: Some("1.94".to_owned()),
            dependencies: Vec::new(),
            targets,
        };
        let refused = package(vec![
            untested("lib"),
            untested("proc-macro"),
            untested("bin"),
            untested("test"),
            gated(&["x"]),
        ]);
        assert_eq!(target_problems("crates/p", &refused, &inherited).len(), 5);
        let clean = package(vec![
            untested("example"),
            untested("bench"),
            gated(&[]),
            target("lib"),
        ]);
        assert!(target_problems("crates/p", &clean, &inherited).is_empty());
    }

    #[test]
    fn the_workspace_edition_and_rust_version_have_no_default() {
        let manifest: Table = "[workspace.package]\nedition = \"2024\"\n".parse().unwrap();
        assert!(matches!(
            inherited(&manifest),
            Err(Failure::Broken(message)) if message == "Cargo.toml has no [workspace.package] rust-version"
        ));
    }

    #[test]
    fn a_build_script_outside_the_listed_crates_is_refused() {
        let package = |kinds: &[&str]| Package {
            name: "p".to_owned(),
            dir: PathBuf::from("/r/crates/p"),
            rust_version: None,
            dependencies: Vec::new(),
            targets: kinds.iter().map(|kind| target(kind)).collect(),
        };
        let scripted = package(&["lib", "custom-build"]);
        assert_eq!(
            build_script_problem("crates/kernel", &scripted),
            Some("crates/kernel: build script custom-build-target is refused: it can set a cfg that hides code from clippy, and only crates/syntax may have one. Move the logic into the crate's code".to_owned())
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
                "crates/syntax/build.rs:2: `unsafe` is unsafe code, and unsafe code belongs in crates/syntax/src/ffi.rs only. Move it into that file",
                "crates/syntax/tests/t.rs:2: `unsafe` is unsafe code, and unsafe code belongs in crates/syntax/src/ffi.rs only. Move it into that file",
                "crates/syntax/src/sub/ffi.rs:2: `unsafe` is unsafe code, and unsafe code belongs in crates/syntax/src/ffi.rs only. Move it into that file",
            ]
        );
    }

    #[test]
    fn global_asm_outside_the_ffi_file_is_a_problem() {
        let text = "core::arch::global_asm!(\"\");\n";
        let files = [
            source("crates/syntax/src/lib.rs", text),
            source("crates/syntax/src/ffi.rs", text),
        ];
        assert_eq!(
            unsafe_problems(&files),
            [
                "crates/syntax/src/lib.rs:1: `global_asm` is unsafe code, and unsafe code belongs in crates/syntax/src/ffi.rs only. Move it into that file"
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
                "crates/syntax/src/ffi.rs:2: macro_rules! is refused here: a macro would carry unsafe out of the file. Write each unsafe item out in full"
            ]
        );
    }
}
