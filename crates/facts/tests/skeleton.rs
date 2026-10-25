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
-- A plain summary line.
-- Its second line.
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

const JSDOC: &str = "\
/**
 * A file header above a blank line.
 */

export class SdkConfig {
    /**
     * Resets the config to be completely empty.
     */
    static unset() {
        SdkConfig.setInstance({});
    }

    /*** A banner, no doc comment. */
    static add() {}

    /** @param cfg the config */
    static put(cfg) {}
}
";

#[test]
fn jsdoc_comments_keep_their_opener_and_first_text_line_with_docstrings() {
    let default = "\
export class SdkConfig {
    static unset() { ... }

    static add() { ... }

    static put(cfg) { ... }
}
";
    let kept = "\
export class SdkConfig {
    /** Resets the config to be completely empty. */
    static unset() { ... }

    static add() { ... }

    /** @param cfg the config */
    static put(cfg) { ... }
}
";
    for language in [Language::Javascript, Language::Typescript, Language::Tsx] {
        let found = skeleton(language, JSDOC, SkeletonOptions::default());
        assert_eq!(found, default, "{language:?}");
        assert_eq!(
            skeleton(language, JSDOC, docstrings()),
            kept,
            "{language:?}"
        );
    }
}

#[test]
fn go_doc_comments_are_the_comments_right_above_a_declaration() {
    let source = "\
// Package shapes draws shapes.
package shapes

// ----------------------------------------------------------------------------
// A section comment above a blank line.

// Area returns the area.
//
// It never fails.
//go:noinline
func Area() int {
\treturn 1
}

//go:generate stringer -type=Kind
type Kind int

// Box holds two sides.
type Box struct {
\t// W is the width.
\tW int // trailing
}

/* Block doc of V. */
var V = 1
";
    let default = "\
package shapes

func Area() int { ... }

type Kind int

type Box struct {
\tW int
}

var V = 1
";
    let kept = "\
// Package shapes draws shapes.
package shapes

// Area returns the area.
func Area() int { ... }

type Kind int

// Box holds two sides.
type Box struct {
\t// W is the width.
\tW int
}

/* Block doc of V. */
var V = 1
";
    let found = skeleton(Language::Go, source, SkeletonOptions::default());
    assert_eq!(found, default);
    assert_eq!(skeleton(Language::Go, source, docstrings()), kept);
}

#[test]
fn a_doc_comment_above_a_closing_brace_documents_nothing() {
    let go = "type T struct {\n\tX int\n\t// Y int (commented out)\n}\n";
    let expected = "type T struct {\n\tX int\n}\n";
    assert_eq!(skeleton(Language::Go, go, docstrings()), expected);
    let javascript = "class A {\n    m() {}\n    /** A note above the closing brace. */\n}\n";
    let expected = "class A {\n    m() { ... }\n}\n";
    assert_eq!(
        skeleton(Language::Javascript, javascript, docstrings()),
        expected
    );
}

#[test]
fn each_doc_comment_of_a_group_takes_the_answer_of_its_own_group() {
    let source = "\
/** Above a blank line. */
/** The same group. */

/** Above code. */
/** Also above code. */
function f() {}
";
    let expected = "\
/** Above code. */
/** Also above code. */
function f() { ... }
";
    assert_eq!(
        skeleton(Language::Javascript, source, docstrings()),
        expected
    );
}

#[test]
fn a_doc_comment_under_another_parent_takes_its_own_answer() {
    let source = "\
class A {
    /** Inside, above a blank line. */

    m() {}
}
/** Outside, above code. */
function f() {}
class B {
    /** Inside, above code. */
    n() {}
}
/** Outside, above a blank line. */

function g() {}
";
    let expected = "\
class A {

    m() { ... }
}
/** Outside, above code. */
function f() { ... }
class B {
    /** Inside, above code. */
    n() { ... }
}

function g() { ... }
";
    assert_eq!(
        skeleton(Language::Javascript, source, docstrings()),
        expected
    );
}

#[test]
fn java_doc_comments_keep_their_marker_and_first_text_line_with_docstrings() {
    let source = "\
/**
 * A license header above a blank line.
 */

package app;

/** A shape. */
@Deprecated
public class Shape {
    /// The area, in Markdown.
    /// Its second line.
    double area() {
        return 0;
    }

    /**
     * <p>
     * The side length.
     */
    int side;
}
";
    let default = "\
package app;

@Deprecated
public class Shape {
    double area() { ... }

    int side;
}
";
    let kept = "\
package app;

/** A shape. */
@Deprecated
public class Shape {
    /// The area, in Markdown.
    double area() { ... }

    /** The side length. */
    int side;
}
";
    let found = skeleton(Language::Java, source, SkeletonOptions::default());
    assert_eq!(found, default);
    assert_eq!(skeleton(Language::Java, source, docstrings()), kept);
}

#[test]
fn csharp_doc_comments_skip_the_lines_that_hold_only_xml_tags() {
    let source = "\
namespace App
{
    /// <summary>
    /// Gets the name.
    /// </summary>
    public class Named
    {
        //// Four slashes are no doc comment.
        public int A;

        /** Block doc of B. */
        public int B;

        /// <param name=\"x\">The x.</param>
        public int F(int x) { return x; }
    }
}
";
    let default = "\
namespace App
{
    public class Named
    {
        public int A;

        public int B;

        public int F(int x) { ... }
    }
}
";
    let kept = "\
namespace App
{
    /// Gets the name.
    public class Named
    {
        public int A;

        /** Block doc of B. */
        public int B;

        /// <param name=\"x\">The x.</param>
        public int F(int x) { ... }
    }
}
";
    let found = skeleton(Language::Csharp, source, SkeletonOptions::default());
    assert_eq!(found, default);
    assert_eq!(skeleton(Language::Csharp, source, docstrings()), kept);
}

#[test]
fn ldoc_comments_open_with_three_hyphens_and_go_on_over_comment_lines() {
    let source = "\
--- A module summary above a blank line.
-- @module shapes

local M = {}

--- Scales a value.
-- More text.
-- @param a the value
function M.scale(a)
  return a * 2
end

-----------------
-- Heading text.
local x = 1

---@param b number
local function inner(b)
  return b
end

--[[-- Block doc of y.
 more ]]
local y = 2
return M
";
    let default = "\
local M = {}

function M.scale(a)
  ...
end

local x = 1

local function inner(b)
  ...
end

local y = 2
return M
";
    let kept = "\
local M = {}

--- Scales a value.
function M.scale(a)
  ...
end

--- Heading text.
local x = 1

--- @param b number
local function inner(b)
  ...
end

--[[-- Block doc of y. ]]
local y = 2
return M
";
    let found = skeleton(Language::Lua, source, SkeletonOptions::default());
    assert_eq!(found, default);
    assert_eq!(skeleton(Language::Lua, source, docstrings()), kept);
}

#[test]
fn a_long_jsdoc_line_is_cut_at_the_docstring_limit() {
    let text = "word ".repeat(60);
    let source = format!("/**\n * {text}\n */\nfunction f() {{}}\n");
    let kept: String = text.chars().take(197).collect();
    let expected = format!("/** {kept}... */\nfunction f() {{ ... }}\n");
    assert_eq!(
        skeleton(Language::Javascript, &source, docstrings()),
        expected
    );
}

#[test]
fn sql_block_comments_go_as_the_line_comments_do() {
    let source = "\
-- A line comment.
/* A block comment. */
CREATE TABLE t (
    id INT, -- trailing line comment
    name TEXT /* trailing block comment */
);

/*
 * A block comment
 * over three lines
 */
SELECT id /* inline */ FROM t;
";
    let expected = "\
CREATE TABLE t (
    id INT,
    name TEXT
);

SELECT id FROM t;
";
    for options in [SkeletonOptions::default(), docstrings()] {
        let found = skeleton(Language::Sql, source, options);
        assert_eq!(found, expected, "{options:?}");
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

#[test]
fn the_closing_tokens_after_an_elided_body_stay_and_a_comment_after_them_goes() {
    let options = SkeletonOptions::default();
    let rust = "impl Area for i32 { fn area(&self) -> f64 { 0.0 } }\nfn f() { 1 } // note\n";
    let expected = "impl Area for i32 { fn area(&self) -> f64 { ... } }\nfn f() { ... }\n";
    assert_eq!(skeleton(Language::Rust, rust, options), expected);
    let javascript = "describe(\"x\", function () {\n  run();\n});\n";
    let expected = "describe(\"x\", function () { ... });\n";
    assert_eq!(
        skeleton(Language::Javascript, javascript, options),
        expected
    );
}

#[test]
fn sql_comment_keywords_and_comment_statements_are_code() {
    let source = "CREATE TABLE users (\n    id INT PRIMARY KEY,\n    name TEXT COMMENT 'the \
                  display name'\n);\n\nCOMMENT ON TABLE users IS 'people who sign in';\n\n-- a \
                  real comment\nCREATE VIEW active AS SELECT id FROM users;\n";
    let expected = "CREATE TABLE users (\n    id INT PRIMARY KEY,\n    name TEXT COMMENT 'the \
                    display name'\n);\n\nCOMMENT ON TABLE users IS 'people who sign in';\n\n\
                    CREATE VIEW active AS SELECT id FROM users;\n";
    assert_eq!(
        skeleton(Language::Sql, source, SkeletonOptions::default()),
        expected
    );
}
