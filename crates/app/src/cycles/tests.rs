use git::Snapshot;
use store::FactSource;

use super::{CyclesRequest, cycles};
use crate::testing::Sandbox;
use crate::{Error, IntArg, Reading, Repo};

fn report(limit: &str, files: &[(&str, &str)]) -> Result<super::CycleReport, Error> {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", files);
    let services = sandbox.services(&root);
    let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
    let snapshot = Snapshot {
        head_sha: None,
        tree_oid: None,
        dirty_count: None,
        note: String::new(),
    };
    let source = FactSource::parsing();
    let reading = Reading {
        repo: &repo,
        snapshot: &snapshot,
        facts: &source,
        deadline: None,
    };
    let request = CyclesRequest {
        limit: IntArg::parse(limit).unwrap_or_else(|| panic!("{limit:?}")),
    };
    cycles(&services, &reading, &request).value
}

const KNOT: [(&str, &str); 3] = [
    ("pkg/__init__.py", ""),
    ("pkg/a.py", "from pkg import b\n"),
    ("pkg/b.py", "from pkg import a\n"),
];

#[test]
fn a_two_file_knot_is_one_cycle_that_closes_on_its_first_file() {
    let found = report("20", &KNOT).unwrap_or_else(|error| panic!("{error}"));
    let rows = found.cycles.rows();
    assert_eq!(found.cycles.total(), 1);
    assert_eq!(rows[0].files, ["pkg/a.py", "pkg/b.py"]);
    assert_eq!(rows[0].members, ["pkg/a.py", "pkg/b.py"]);
    assert!(found.skipped.is_empty());
}

#[test]
fn the_limit_is_checked_before_the_scan() {
    let Err(Error::Kind(kernel::Error::OperationFailed(message))) = report("0", &KNOT) else {
        panic!("a bound refusal");
    };
    assert_eq!(message, "limit takes a value from 1 through 500, got 0");
}
