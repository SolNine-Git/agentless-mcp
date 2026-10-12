use super::{in_metric_scope, is_fixture_path, is_test_path};
use crate::testing::cases;

#[test]
fn each_path_rule_matches_the_python_rule() {
    let rows: Vec<(String, bool, bool, bool)> = cases("scope", "paths");
    for (path, test, fixture, metric) in rows {
        assert_eq!(is_test_path(&path), test, "test: {path:?}");
        assert_eq!(is_fixture_path(&path), fixture, "fixture: {path:?}");
        assert_eq!(in_metric_scope(&path), metric, "metric scope: {path:?}");
    }
}
