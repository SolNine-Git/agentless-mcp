//! Every skeleton case of the fixtures in its four option sets, and the 10 byte goldens of the
//! characterization suite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use facts::{SkeletonOptions, skeletonize};
use serde::Deserialize;
use syntax::{Language, parse};
use text::testing::cases;

#[derive(Deserialize)]
struct Source {
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    default: String,
    docstrings: String,
    numbers: String,
    docstrings_numbers: String,
}

const OPTIONS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

#[test]
fn every_language_gives_the_reference_skeletons() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for language in Language::ALL {
        let sources = sources(language);
        let file = format!("skeleton_{}", language.name());
        let all: Vec<Case> = cases(env!("CARGO_MANIFEST_DIR"), &file, "cases");
        for case in &all {
            checked += OPTIONS.len();
            problems.extend(differences(language, &sources[&case.name], case));
        }
    }
    assert!(checked > 1300, "the fixtures hold {checked} skeletons");
    let shown: Vec<&String> = problems.iter().take(6).collect();
    assert!(
        problems.is_empty(),
        "{} skeletons differ:\n{}",
        problems.len(),
        shown
            .iter()
            .map(|text| text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_ten_characterization_goldens_are_byte_equal() {
    let repositories = [
        ("repo_py", Language::Python, ".py"),
        ("repo_go", Language::Go, ".go"),
        ("repo_ts", Language::Typescript, ".ts"),
    ];
    let mut compared = 0;
    for (repository, language, suffix) in repositories {
        let directory = characterization().join("fixtures").join(repository);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
            .map(|entry| entry.expect("a directory entry").path())
            .filter(|path| path.to_string_lossy().ends_with(suffix))
            .collect();
        files.sort();
        for file in files {
            let name = file.file_name().expect("a file name").to_string_lossy();
            let golden = characterization()
                .join("goldens")
                .join(repository)
                .join(format!("{name}.skel"));
            let source = read(&file);
            let found = skeleton(language, &source, SkeletonOptions::default());
            assert_eq!(found, read(&golden), "{}", golden.display());
            compared += 1;
        }
    }
    assert_eq!(compared, 10);
}

const RUST_DOCS: &str = "\
//! The crate's summary line.
//! Its second line.

/// The meta row of an index: its schema version, its repository, its generation, and the
/// analysis its rows were made under.
#[derive(Clone, Debug)]
pub struct IndexMeta {
    /// The schema version that the database records.
    pub schema: i64,
    ///
    /// A run whose first line holds no text.
    pub generation: String,
}

/**
 * A block doc comment over lines.
 */
pub enum Shape { Circle }

/*! An inner block doc. */
fn plain() {}
";

#[test]
fn rust_doc_comments_go_whole_by_default() {
    let expected = "\
#[derive(Clone, Debug)]
pub struct IndexMeta {
    pub schema: i64,
    pub generation: String,
}

pub enum Shape { Circle }

fn plain() { ... }
";
    let found = skeleton(Language::Rust, RUST_DOCS, SkeletonOptions::default());
    assert_eq!(found, expected);
}

#[test]
fn rust_doc_comments_keep_their_marker_and_first_text_line_with_docstrings() {
    let expected = "\
//! The crate's summary line.

/// The meta row of an index: its schema version, its repository, its generation, and the
#[derive(Clone, Debug)]
pub struct IndexMeta {
    /// The schema version that the database records.
    pub schema: i64,
    /// A run whose first line holds no text.
    pub generation: String,
}

/** A block doc comment over lines. */
pub enum Shape { Circle }

/*! An inner block doc. */
fn plain() { ... }
";
    let found = skeleton(Language::Rust, RUST_DOCS, docstrings());
    assert_eq!(found, expected);
}

#[test]
fn an_attribute_and_a_field_under_a_doc_comment_print_whole() {
    let source = "\
pub struct Row {
    /// The key.
    #[serde(rename = \"k\")]
    pub key: String,
}
";
    for options in [SkeletonOptions::default(), docstrings()] {
        let found = skeleton(Language::Rust, source, options);
        let lines: Vec<&str> = found.lines().collect();
        assert!(
            lines.contains(&"    #[serde(rename = \"k\")]"),
            "{options:?}\n{found}"
        );
        assert!(
            lines.contains(&"    pub key: String,"),
            "{options:?}\n{found}"
        );
    }
}

#[test]
fn a_long_doc_comment_line_is_cut_at_the_docstring_limit() {
    let text = "word ".repeat(60);
    let source = format!("/// {text}\nfn f() {{}}\n");
    let kept: String = text.chars().take(197).collect();
    let expected = format!("/// {kept}...\nfn f() {{ ... }}\n");
    assert_eq!(skeleton(Language::Rust, &source, docstrings()), expected);
}

#[test]
fn lua_comments_go_whole_in_both_modes() {
    let source = "\
--- A LuaDoc summary line.
--- Its second line.
local function scale(a)
  -- inside the body
  return a * 2
end

--[[ A block
comment ]]
local x = 1 -- trailing
return x
";
    let expected = "\
local function scale(a)
  ...
end

local x = 1
return x
";
    for options in [SkeletonOptions::default(), docstrings()] {
        assert_eq!(
            skeleton(Language::Lua, source, options),
            expected,
            "{options:?}"
        );
    }
}

fn docstrings() -> SkeletonOptions {
    SkeletonOptions {
        docstrings: true,
        number_lines: false,
    }
}

fn differences(language: Language, source: &str, case: &Case) -> Vec<String> {
    let expected = [
        &case.default,
        &case.docstrings,
        &case.numbers,
        &case.docstrings_numbers,
    ];
    OPTIONS
        .into_iter()
        .zip(expected)
        .filter_map(|((docstrings, number_lines), expected)| {
            let options = SkeletonOptions {
                docstrings,
                number_lines,
            };
            let found = skeleton(language, source, options);
            (found != *expected).then(|| {
                format!(
                    "{} {options:?}\n--- found\n{found}--- expected\n{expected}",
                    case.name
                )
            })
        })
        .collect()
}

fn sources(language: Language) -> HashMap<String, String> {
    let file = format!("extract_{}", language.name());
    let all: Vec<Source> = cases(env!("CARGO_MANIFEST_DIR"), &file, "cases");
    all.into_iter()
        .map(|source| (source.name, source.source))
        .collect()
}

fn skeleton(language: Language, source: &str, options: SkeletonOptions) -> String {
    let parsed = parse(language, Arc::from(source)).expect("each fixture source parses");
    skeletonize(&parsed, options)
}

fn characterization() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/characterization")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}
