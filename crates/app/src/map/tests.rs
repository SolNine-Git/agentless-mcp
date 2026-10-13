use std::path::{Path, PathBuf};

use git::Snapshot;
use io::{Granularity, ProjectConfig};
use store::FactSource;

use super::{MapRequest, MapResult, Shape, map};
use crate::scan::RepoFacts;
use crate::testing::Sandbox;
use crate::{Error, IntArg, Reading, Repo, Services};

const FILES: [(&str, &str); 4] = [
    ("pkg/__init__.py", ""),
    (
        "pkg/core.py",
        "class Engine:\n    def run(self):\n        return 1\n",
    ),
    (
        "pkg/use.py",
        "from pkg.core import Engine\n\ndef go():\n    return Engine().run()\n",
    ),
    (
        "tests/test_core.py",
        "from pkg.core import Engine\n\ndef test_engine():\n    Engine()\n",
    ),
];

pub(super) fn int(text: &str) -> IntArg {
    IntArg::parse(text).unwrap_or_else(|| panic!("{text:?}"))
}

pub(super) fn request(focus: &[String], granularity: Granularity) -> MapRequest {
    MapRequest {
        focus: focus.to_vec(),
        budget: None,
        max_files: None,
        granularity: Some(granularity),
    }
}

// Runs `read` over the repository at `root` with no cache, and with a HEAD only when `head` is
// given, as the golden harness's context has none.
fn reading_at<T>(
    root: &Path,
    head: Option<&str>,
    read: impl FnOnce(&Services, &Reading<'_>) -> T,
) -> T {
    let sandbox = Sandbox::new();
    let services = sandbox.services(root);
    let repo = Repo::load(&services, root).unwrap_or_else(|error| panic!("{error}"));
    let snapshot = Snapshot {
        head_sha: head.map(str::to_owned),
        tree_oid: None,
        dirty_count: None,
        note: String::new(),
    };
    let source = FactSource::parsing();
    let reading = Reading {
        repo: &repo,
        snapshot: &snapshot,
        facts: &source,
    };
    read(&services, &reading)
}

pub(super) fn fixture_root(repo: &str) -> PathBuf {
    let parts = [
        env!("CARGO_MANIFEST_DIR"),
        "../../tests/characterization/fixtures",
        repo,
    ];
    parts.iter().collect()
}

// The scan of one characterization fixture repository.
pub(super) fn fixture_facts(repo: &str) -> RepoFacts {
    let loaded = reading_at(&fixture_root(repo), None, RepoFacts::load);
    assert!(
        loaded.diagnostics.is_empty(),
        "{repo}: {:?}",
        loaded.diagnostics
    );
    loaded
        .value
        .unwrap_or_else(|error| panic!("{repo}: {error}"))
}

pub(super) fn fixture_map(repo: &str, request: &MapRequest) -> MapResult {
    let outcome = reading_at(&fixture_root(repo), None, |services, reading| {
        map(services, reading, request).value
    });
    outcome.unwrap_or_else(|error| panic!("{repo}: {error}"))
}

fn mapped(request: &MapRequest, head: Option<&str>) -> Result<MapResult, Error> {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &FILES);
    reading_at(&root, head, |services, reading| {
        map(services, reading, request).value
    })
}

fn refusal(shape: Result<Shape, Error>) -> String {
    match shape {
        Err(Error::Kind(kernel::Error::OperationFailed(message))) => message,
        _ => panic!("a bound refusal"),
    }
}

#[test]
fn the_file_cap_and_the_budget_are_checked_before_the_scan() {
    let config = ProjectConfig::default();
    let mut asked = request(&[], Granularity::Function);
    asked.max_files = Some(int("0"));
    let expected = "max_files takes a value from 1 through 200, got 0";
    assert_eq!(refusal(Shape::of(&asked, &config)), expected);
    asked.max_files = None;
    asked.budget = Some(int("1"));
    let expected = "budget takes a value from 200 through 64000, got 1";
    assert_eq!(refusal(Shape::of(&asked, &config)), expected);
}

#[test]
fn the_config_fills_the_cap_and_the_granularity_the_request_leaves_unset() {
    let config = ProjectConfig {
        max_files: Some(3),
        granularity: Some(Granularity::File),
        ..ProjectConfig::default()
    };
    let mut asked = request(&[], Granularity::Function);
    asked.granularity = None;
    let shape = Shape::of(&asked, &config).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((shape.max_files, shape.budget), (3, None));
    assert_eq!(shape.granularity, Granularity::File);
}

#[test]
fn a_focused_map_seeds_its_file_and_names_the_entry_that_seeded_nothing() {
    // Two files keep the test out of the ranking, which is where a companion comes from.
    let focus = ["pkg/core.py".to_owned(), "no_such_name_xyz".to_owned()];
    let mut asked = request(&focus, Granularity::Function);
    asked.max_files = Some(int("2"));
    let result = mapped(&asked, None).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.seeds, ["pkg/core.py"]);
    assert_eq!(result.unresolved_seeds, ["no_such_name_xyz"]);
    assert_eq!(result.files[0].path, "pkg/core.py");
    assert!(result.included > 0 && result.included <= result.candidates);
    assert_eq!(result.expand_order.len(), result.included);
    let rows = result.companions.rows.rows();
    let paths: Vec<&str> = rows.iter().map(|row| row.path.as_str()).collect();
    assert_eq!(paths, ["tests/test_core.py"]);
}

#[test]
fn a_file_map_counts_no_symbol_and_churn_needs_a_head() {
    let asked = request(&[], Granularity::File);
    let bare = mapped(&asked, None).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((bare.budget, bare.included, bare.candidates), (0, 0, 0));
    let unstamped =
        |file: &super::MapFile| file.entries.is_empty() && file.commits_window.is_none();
    assert!(bare.files.iter().all(unstamped));
    let stamped = mapped(&asked, Some("0000000f")).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        stamped
            .files
            .iter()
            .all(|file| file.commits_window.is_some())
    );
}
