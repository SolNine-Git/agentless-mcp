//! Each command line of the parser fixtures: help texts at many widths, usage errors, and the
//! refusals that come before any repository work. crates/cli/fixtures/parser.json holds the
//! top parser, lint and capabilities; crates/cli/fixtures/read/parser.json holds the read
//! subcommands.

use std::ffi::OsString;

use crate::common::{Sandbox, cases, expected, words};

// Runs every case of `file` and fails with each one that differs.
fn every_run_answers_as_the_fixture(file: &str, at_least: usize) {
    let sandbox = Sandbox::new();
    let runs = cases(file, "runs");
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
    assert!(runs.len() > at_least, "{file} holds {} cases", runs.len());
    assert!(
        failed.is_empty(),
        "{} cases differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn every_parser_case_answers_as_the_fixture() {
    every_run_answers_as_the_fixture("parser", 150);
}

#[test]
fn every_read_parser_case_answers_as_the_fixture() {
    every_run_answers_as_the_fixture("read/parser", 100);
}
