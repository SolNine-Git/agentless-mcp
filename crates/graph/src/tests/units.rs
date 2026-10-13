use serde::Deserialize;
use text::testing::cases;

use crate::imports::{resolve_import_target, resolve_imported_submodule};
use crate::paths::{PathIndex, module_stems, module_tail, normalized, normpath, stem};
use crate::unresolved::{names_this_repository, repository_segments};

const UNITS: &str = "units";

fn unit_cases<T: serde::de::DeserializeOwned>(section: &str) -> Vec<T> {
    cases(env!("CARGO_MANIFEST_DIR"), UNITS, section)
}

fn unit_paths() -> Vec<String> {
    unit_cases::<Vec<String>>("unit_paths").remove(0)
}

#[test]
fn normpath_and_normalized_match_the_reference() {
    for (text, collapsed, root_empty) in unit_cases::<(String, String, String)>("normpath") {
        assert_eq!(normpath(&text), collapsed, "normpath {text:?}");
        assert_eq!(normalized(&text), root_empty, "normalized {text:?}");
    }
}

#[test]
fn stems_match_the_reference() {
    for (path, expected, stems) in unit_cases::<(String, String, Vec<String>)>("stems") {
        assert_eq!(stem(&path), expected, "stem {path:?}");
        assert_eq!(module_stems(&path), stems, "module stems {path:?}");
    }
}

#[test]
fn module_tails_match_the_reference() {
    for (module, tail) in unit_cases::<(String, String)>("module_tails") {
        assert_eq!(module_tail(&module).unwrap_or_default(), tail, "{module:?}");
    }
}

#[test]
fn suffix_matches_match_the_reference() {
    let owned = unit_paths();
    let paths = PathIndex::build(owned.iter().map(String::as_str));
    for (module, expected) in unit_cases::<(String, Option<String>)>("suffix_match") {
        assert_eq!(
            paths.suffix_match(&module),
            expected.as_deref(),
            "{module:?}"
        );
    }
}

#[test]
fn repository_segments_match_the_reference() {
    let (paths, expected) = unit_cases::<(Vec<String>, Vec<String>)>("segments").remove(0);
    let segments = repository_segments(paths.iter().map(String::as_str));
    assert_eq!(segments.iter().copied().collect::<Vec<_>>(), expected);
    for (module, named) in unit_cases::<(String, bool)>("names_this_repository") {
        assert_eq!(
            names_this_repository(&module, &segments),
            named,
            "{module:?}"
        );
    }
}

#[derive(Deserialize)]
struct Statement {
    module: String,
    names: Vec<String>,
    relative_level: u32,
}

#[derive(Deserialize)]
struct ImportCase {
    importer: String,
    statement: Statement,
    target: Option<String>,
    submodules: Vec<(String, Option<String>)>,
}

#[test]
fn import_targets_match_the_reference() {
    let owned = unit_paths();
    let paths = PathIndex::build(owned.iter().map(String::as_str));
    for case in unit_cases::<ImportCase>("imports") {
        let (importer, statement) = (&case.importer, &case.statement);
        let found = resolve_import_target(
            importer,
            &statement.module,
            statement.relative_level,
            &paths,
        );
        assert_eq!(
            found,
            case.target.as_deref(),
            "{importer} {:?}",
            statement.module
        );
        let names: Vec<(String, Option<String>)> = statement
            .names
            .iter()
            .map(|name| {
                let module = &statement.module;
                let level = statement.relative_level;
                let found = resolve_imported_submodule(importer, module, level, name, &paths);
                (name.clone(), found.map(str::to_owned))
            })
            .collect();
        assert_eq!(names, case.submodules, "{importer} {:?}", statement.module);
    }
}
