//! The path rules that decide which files each pair check and the clone index read.

use syntax::{Language, Tier};
use text::{posix_name, split_suffix};

const TEST_DIRECTORIES: [&str; 6] = ["test", "tests", "testing", "spec", "specs", "__tests__"];
const FIXTURE_DIRECTORIES: [&str; 2] = ["fixtures", "testdata"];
const RUST_TEST_STEMS: [&str; 2] = ["tests", "testing"];
const TEST_PREFIX: &str = "test_";
const TEST_SUFFIXES: [&str; 11] = [
    "_test",
    "_tests",
    "_spec",
    ".test",
    ".spec",
    "-test",
    "-spec",
    "Test",
    "Tests",
    "Spec",
    "_unittest",
];

/// Returns true when `path` is a test file, by a directory segment or by its file name.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    if has_directory(path, &TEST_DIRECTORIES) {
        return true;
    }
    let file = path.rsplit_once('/').map_or(path, |(_, file)| file);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    stem == "conftest"
        || stem.ends_with("_test")
        || file.contains(".test.")
        || file.contains(".spec.")
        || is_rust_test_module(path, stem)
}

/// Returns the stem that a test file's name points at, without its test affix: None for no affix
/// or an empty rest.
#[must_use]
pub fn test_subject(path: &str) -> Option<&str> {
    let (stem, _) = split_suffix(posix_name(path));
    let suffixed = || (TEST_SUFFIXES.iter()).find_map(|suffix| stem.strip_suffix(suffix));
    let rest = stem.strip_prefix(TEST_PREFIX).or_else(suffixed);
    rest.filter(|rest| !rest.is_empty())
}

/// Returns true when a directory segment of `path` names a fixture directory.
#[must_use]
pub fn is_fixture_path(path: &str) -> bool {
    has_directory(path, &FIXTURE_DIRECTORIES)
}

/// Returns true when `path` is a tier-1 source file outside test and fixture paths.
#[must_use]
pub fn in_metric_scope(path: &str) -> bool {
    Language::from_path(path).is_some_and(|language| language.tier() == Tier::One)
        && !is_test_path(path)
        && !is_fixture_path(path)
}

// The file that `#[cfg(test)] mod tests;` loads, and the crate's test helpers in `testing.rs`.
fn is_rust_test_module(path: &str, stem: &str) -> bool {
    RUST_TEST_STEMS.contains(&stem) && Language::from_path(path) == Some(Language::Rust)
}

// Whole segments only, and the file name is not a directory.
fn has_directory(path: &str, names: &[&str]) -> bool {
    let Some((directories, _)) = path.rsplit_once('/') else {
        return false;
    };
    directories
        .split('/')
        .any(|segment| names.contains(&segment))
}

#[cfg(test)]
mod tests;
