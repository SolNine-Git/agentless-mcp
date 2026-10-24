use serde_json::Value;

use super::rows::{cases, report};
use crate::lint::render_lint;

#[test]
fn each_report_renders_as_the_fixture_says() {
    let all: Vec<(Vec<Value>, String)> = cases("lint", "render");
    for (rows, expected) in all {
        assert_eq!(render_lint(&report(&rows)), expected, "{rows:?}");
    }
}
