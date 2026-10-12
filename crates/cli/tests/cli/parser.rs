//! Each command line of crates/cli/fixtures/parser.json: help texts at many widths, usage
//! errors, and the refusals that come before any repository work.

use std::ffi::OsString;

use crate::common::{Sandbox, cases, expected, words};

#[test]
fn every_parser_case_answers_as_the_fixture() {
    let sandbox = Sandbox::new();
    let runs = cases("parser", "runs");
    let mut failed = Vec::new();
    for case in &runs {
        let argv: Vec<OsString> = words(&case["argv"])
            .into_iter()
            .map(OsString::from)
            .collect();
        let columns = case["columns"].as_str();
        let got = sandbox.run(&argv, sandbox.path(), columns);
        let want = expected(case);
        if got != want {
            failed.push(format!(
                "{argv:?} COLUMNS={columns:?}\n got {got:?}\nwant {want:?}"
            ));
        }
    }
    assert!(runs.len() > 150, "the fixture holds {} cases", runs.len());
    assert!(
        failed.is_empty(),
        "{} cases differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
