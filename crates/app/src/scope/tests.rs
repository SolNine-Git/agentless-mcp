use super::{in_metric_scope, is_fixture_path, is_test_path};
use crate::testing::MANIFEST_DIR;

#[test]
fn each_path_rule_matches_the_reference_rule() {
    let rows: Vec<(String, bool, bool, bool)> = io::testing::cases(MANIFEST_DIR, "scope", "paths");
    for (path, test, fixture, metric) in rows {
        assert_eq!(is_test_path(&path), test, "test: {path:?}");
        assert_eq!(is_fixture_path(&path), fixture, "fixture: {path:?}");
        assert_eq!(in_metric_scope(&path), metric, "metric scope: {path:?}");
    }
}

#[test]
fn a_rust_file_with_the_stem_tests_or_testing_is_a_test_path() {
    let cases = [
        ("crates/git/src/worktree/tests.rs", true),
        ("crates/app/src/testing.rs", true),
        ("tests.rs", true),
        ("src/lib.rs", false),
        ("src/table_tests.rs", false),
        ("src/tests.RS", false),
        ("src/tests.rs.orig", false),
        ("django/contrib/admin/tests.py", false),
        ("xarray/testing.py", false),
        ("dronegen/tests.go", false),
        (".github/workflows/tests.yml", false),
    ];
    for (path, test) in cases {
        assert_eq!(is_test_path(path), test, "test: {path:?}");
    }
    assert!(!in_metric_scope("crates/app/src/testing.rs"));
}
