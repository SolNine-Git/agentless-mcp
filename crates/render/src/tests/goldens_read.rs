use std::path::{Path, PathBuf};

use app::{CyclesRequest, EnvConfig, IntArg, MapRequest, Reading, Repo, Services, TreeRequest};
use git::Snapshot;
use io::testing::Tree;
use store::FactSource;

use crate::{Receipt, View, json, text};

const PLACEHOLDER: &str = "<REPO>";

fn characterization(parts: &[&str]) -> PathBuf {
    let base = [env!("CARGO_MANIFEST_DIR"), "../../tests/characterization"];
    base.iter().chain(parts).collect()
}

// A characterization golden, read where it is committed.
pub(super) fn golden(kind: &str, repo: &str, name: &str) -> String {
    let path = characterization(&["goldens", kind, &format!("{repo}.{name}")]);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// One fixture repository with the pinned git state of the golden harness and no cache.
pub(super) struct Pinned {
    _home: Tree,
    pub(super) services: Services,
    repo: Repo,
    snapshot: Snapshot,
    source: FactSource,
}

impl Pinned {
    pub(super) fn new(repo: &str) -> Pinned {
        let home = Tree::new().unwrap_or_else(|error| panic!("a home: {error}"));
        let root = characterization(&["fixtures", repo]);
        let env = EnvConfig::new(io::testing::hermetic_env(home.path()), root.clone());
        let services = Services::new(&env);
        let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
        let snapshot = Snapshot {
            head_sha: Some("0000000f".to_owned()),
            tree_oid: Some("1111111f".to_owned()),
            dirty_count: Some(0),
            note: String::new(),
        };
        Pinned {
            _home: home,
            services,
            repo,
            snapshot,
            source: FactSource::parsing(),
        }
    }

    pub(super) fn reading(&self) -> Reading<'_> {
        Reading {
            repo: &self.repo,
            snapshot: &self.snapshot,
            facts: &self.source,
            deadline: None,
        }
    }

    // The answer as the golden harness wraps it, with the root as its placeholder.
    pub(super) fn text(&self, view: &impl View) -> String {
        let receipt = Receipt::new(&self.repo, &self.snapshot, "none");
        let root = root_text(&self.repo.root);
        text(&receipt, view).replace(&root, PLACEHOLDER)
    }

    pub(super) fn json(&self, view: &impl View) -> String {
        let receipt = Receipt::new(&self.repo, &self.snapshot, "none");
        let root = root_text(&self.repo.root);
        let document = json(&receipt, view).unwrap_or_else(|error| panic!("{error}"));
        document.replace(&root, PLACEHOLDER)
    }
}

fn root_text(root: &Path) -> String {
    root.display().to_string()
}

pub(super) fn int(text: &str) -> IntArg {
    IntArg::parse(text).unwrap_or_else(|| panic!("{text:?}"))
}

#[test]
fn the_tree_goldens_come_out_byte_for_byte() {
    for repo in ["repo_py", "repo_ts", "repo_go", "repo_py_tests"] {
        let pinned = Pinned::new(repo);
        let request = TreeRequest {
            depth: int("4"),
            max_entries: int("500"),
        };
        let noted = app::tree(&pinned.services, &pinned.repo, &request);
        assert!(
            noted.diagnostics.is_empty(),
            "{repo}: {:?}",
            noted.diagnostics
        );
        let view = noted
            .value
            .unwrap_or_else(|error| panic!("{repo}: {error}"));
        assert_eq!(
            pinned.text(&view),
            golden("map", repo, "tree.txt"),
            "{repo}"
        );
    }
}

#[test]
fn the_map_goldens_come_out_byte_for_byte() {
    for repo in ["repo_py", "repo_ts", "repo_go", "repo_py_tests"] {
        let pinned = Pinned::new(repo);
        // The harness's context has no churn source, and a snapshot without a head runs no log.
        let bare = Snapshot {
            head_sha: None,
            ..pinned.snapshot.clone()
        };
        let reading = Reading {
            snapshot: &bare,
            ..pinned.reading()
        };
        let request = MapRequest {
            focus: Vec::new(),
            budget: None,
            max_files: None,
            granularity: None,
        };
        let noted = app::map(&pinned.services, &reading, &request);
        assert!(
            noted.diagnostics.is_empty(),
            "{repo}: {:?}",
            noted.diagnostics
        );
        let result = noted
            .value
            .unwrap_or_else(|error| panic!("{repo}: {error}"));
        assert_eq!(
            pinned.text(&result),
            golden("map", repo, "map.txt"),
            "{repo}"
        );
        assert_eq!(
            pinned.json(&result),
            golden("map", repo, "map.json"),
            "{repo}"
        );
    }
}

#[test]
fn the_cycles_goldens_come_out_byte_for_byte() {
    for repo in ["repo_py", "repo_ts", "repo_nested"] {
        let pinned = Pinned::new(repo);
        let request = CyclesRequest { limit: int("20") };
        let noted = app::cycles(&pinned.services, &pinned.reading(), &request);
        assert!(
            noted.diagnostics.is_empty(),
            "{repo}: {:?}",
            noted.diagnostics
        );
        let report = noted
            .value
            .unwrap_or_else(|error| panic!("{repo}: {error}"));
        assert_eq!(
            pinned.text(&report),
            golden("graph", repo, "cycles.txt"),
            "{repo}"
        );
    }
}
