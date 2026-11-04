//! The path rules that decide which files each pair check and the clone index read.

use syntax::{Language, Tier};

const TEST_DIRECTORIES: [&str; 6] = ["test", "tests", "testing", "spec", "specs", "__tests__"];
const FIXTURE_DIRECTORIES: [&str; 2] = ["fixtures", "testdata"];
const RUST_TEST_STEMS: [&str; 2] = ["tests", "testing"];

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
