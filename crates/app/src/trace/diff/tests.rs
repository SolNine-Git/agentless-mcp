use std::path::Path;

use git::Snapshot;
use store::FactSource;

use super::DiffTarget;
use crate::testing::Sandbox;
use crate::trace::{
    Card, CardKind, DiffSection, DiffTrace, FileStatus, Outcome, Section, Trace, TraceRequest,
    trace, trace_at,
};
use crate::{Error, Reading, Repo, Services};

// One commit past the base edits `load`, drops `gone` and adds `fresh`; the working tree moves
// `shifted` to an untracked file and edits a data key and the test.
const BASE: [(&str, &str); 5] = [
    ("pkg/__init__.py", ""),
    (
        "pkg/svc.py",
        "def load(path):\n    return path\n\n\ndef helper():\n    return load(1)\n\n\n\
         def gone():\n    return 0\n",
    ),
    (
        "pkg/move_from.py",
        "def shifted(a):\n    total = a + 1\n    return total\n",
    ),
    ("conf.json", "{\n  \"name\": \"x\",\n  \"size\": 1\n}\n"),
    (
        "tests/test_svc.py",
        "from pkg.svc import load\n\n\ndef test_load():\n    assert load(1) == 1\n",
    ),
];
const HEAD_SVC: &str = "def load(path):\n    return str(path)\n\n\ndef helper():\n    \
                        return load(1)\n\n\ndef fresh():\n    return 2\n";

struct Fixture {
    _sandbox: Sandbox,
    services: Services,
    repo: Repo,
    snapshot: Snapshot,
}

impl Fixture {
    fn new() -> Fixture {
        let sandbox = Sandbox::new();
        let root = sandbox.repo("repo", &BASE);
        sandbox.git(&root, &["tag", "base"]);
        sandbox.put("repo/pkg/svc.py", HEAD_SVC);
        sandbox.git(&root, &["commit", "--quiet", "-am", "edit load"]);
        working(&sandbox, &root);
        Fixture::of(sandbox, &root)
    }

    fn of(sandbox: Sandbox, root: &Path) -> Fixture {
        let services = sandbox.services(root);
        let repo = Repo::load(&services, root).unwrap_or_else(|error| panic!("{error}"));
        let snapshot = services.git.snapshot(root, Some(root));
        Fixture {
            _sandbox: sandbox,
            services,
            repo,
            snapshot,
        }
    }

    fn traced(
        &self,
        targets: &[&str],
        more: Option<Section>,
        rev: Option<&str>,
    ) -> Result<Trace, Error> {
        let source = FactSource::parsing();
        let reading = Reading {
            repo: &self.repo,
            snapshot: &self.snapshot,
            facts: &source,
            deadline: None,
        };
        let request = TraceRequest {
            targets: targets.iter().map(|target| (*target).to_owned()).collect(),
            more,
        };
        match rev {
            None => trace(&self.services, &reading, &request).value,
            Some(rev) => trace_at(&self.services, &reading, &request, Some(rev)).value,
        }
    }

    fn diff(&self, target: &str, more: Option<Section>) -> DiffTrace {
        let traced = self.traced(&[target], more, None);
        let mut traced = traced.unwrap_or_else(|error| panic!("{error}"));
        match traced.cards.remove(0).outcome {
            Outcome::Card(Card::Diff(card)) => *card,
            other => panic!("{target}: a diff card, got {other:?}"),
        }
    }

    fn refusal(&self, target: &str, rev: Option<&str>) -> String {
        let traced = self.traced(&[target], None, rev);
        traced
            .map(|_| ())
            .map_err(|error| error.to_string())
            .unwrap_err()
    }
}

fn working(sandbox: &Sandbox, root: &Path) {
    sandbox.git(root, &["rm", "--quiet", "pkg/move_from.py"]);
    let moved = "def shifted(a):\n    total = a + 1\n    return total\n";
    sandbox.put("repo/pkg/move_to.py", format!("X = 1\n\n\n{moved}"));
    sandbox.put(
        "repo/conf.json",
        "{\n  \"name\": \"x\",\n  \"size\": 2\n}\n",
    );
    let test = "from pkg.svc import load\n\n\ndef test_load():\n    assert load(1) == \"1\"\n";
    sandbox.put("repo/tests/test_svc.py", test);
}

fn ids(section: &DiffSection) -> Vec<&str> {
    (section.rows.iter())
        .map(|row| row.card.stable_id.as_str())
        .collect()
}

#[test]
fn a_diff_target_names_its_revision_then_its_path() {
    let parse = |target| DiffTarget::parse(target).map(|found| (found.rev, found.scope));
    let owned = |rev: &str, scope: Option<&str>| Some((rev.to_owned(), scope.map(str::to_owned)));
    assert_eq!(parse("diff:main"), owned("main", None));
    assert_eq!(
        parse("diff:main:crates/app/"),
        owned("main", Some("crates/app"))
    );
    assert_eq!(parse("diff:v1.2:./a.py"), owned("v1.2", Some("a.py")));
    assert_eq!(parse("diff:main:"), owned("main", None));
    assert_eq!(parse("py:a.py::diff"), None);
}

#[test]
fn the_card_lists_the_files_and_symbols_since_the_merge_base() {
    let fixture = Fixture::new();
    let card = fixture.diff("diff:base", None);
    assert_eq!(
        (card.rev.as_str(), card.commits, card.full),
        ("base", 1, false)
    );
    let files = &card.files;
    let counts = (
        files.total,
        files.added,
        files.removed,
        files.changed,
        files.renamed,
    );
    assert_eq!(counts, (5, 1, 1, 3, 0));
    let folders: Vec<(&str, usize)> = (files.folders.iter())
        .map(|folder| (folder.path.as_str(), folder.files))
        .collect();
    assert_eq!(folders, [("pkg", 3), ("", 1), ("tests", 1)]);
    assert_eq!(files.rows, None);
    let symbols = card.symbols.unwrap_or_else(|| panic!("the sections"));
    assert_eq!(
        ids(&symbols.changed),
        ["py:pkg/svc.py::load", "py:tests/test_svc.py::test_load"]
    );
    assert_eq!((symbols.changed.files, symbols.changed.tests), (2, 1));
    assert_eq!(symbols.changed.rows[0].base, Some((1, 2)));
    assert_eq!(
        ids(&symbols.added),
        ["py:pkg/svc.py::fresh", "py:pkg/move_to.py::X"]
    );
    assert_eq!(ids(&symbols.removed), ["py:pkg/svc.py::gone"]);
    let moved: Vec<(&str, &str)> = (symbols.moved.rows.iter())
        .map(|row| (row.from.stable_id.as_str(), row.to.stable_id.as_str()))
        .collect();
    assert_eq!(
        moved,
        [("py:pkg/move_from.py::shifted", "py:pkg/move_to.py::shifted")]
    );
}

#[test]
fn a_data_file_counts_as_a_file_and_lists_its_keys_only_when_scoped() {
    let fixture = Fixture::new();
    let card = fixture.diff("diff:base", None);
    let root = card
        .files
        .folders
        .iter()
        .find(|folder| folder.path.is_empty());
    let root = root.unwrap_or_else(|| panic!("the root folder"));
    assert_eq!((root.files, root.symbols.changed), (1, 0));
    let scoped = fixture.diff("diff:base:conf.json", None);
    assert!(scoped.full);
    let symbols = scoped.symbols.unwrap_or_else(|| panic!("the sections"));
    assert_eq!(ids(&symbols.changed), ["json:conf.json::size"]);
}

#[test]
fn more_tree_lists_each_file_and_more_declares_each_row() {
    let fixture = Fixture::new();
    let tree = fixture.diff("diff:base", Some(Section::Tree));
    assert!(tree.symbols.is_none());
    let rows = tree.files.rows.unwrap_or_else(|| panic!("the file rows"));
    let listed: Vec<(&str, &FileStatus)> = (rows.iter())
        .map(|row| (row.path.as_str(), &row.status))
        .collect();
    assert_eq!(
        listed,
        [
            ("conf.json", &FileStatus::Changed),
            ("pkg/move_from.py", &FileStatus::Removed),
            ("pkg/move_to.py", &FileStatus::Added),
            ("pkg/svc.py", &FileStatus::Changed),
            ("tests/test_svc.py", &FileStatus::Changed),
        ]
    );
    let declares = fixture.diff("diff:base", Some(Section::Declares));
    assert!(declares.full);
    let symbols = declares.symbols.unwrap_or_else(|| panic!("the sections"));
    assert_eq!(symbols.added.rows.len(), symbols.added.total);
}

#[test]
fn a_section_that_a_diff_card_lacks_gets_the_no_section_answer() {
    let fixture = Fixture::new();
    let traced = fixture.traced(&["diff:base"], Some(Section::Callers), None);
    let traced = traced.unwrap_or_else(|error| panic!("{error}"));
    let expected = Outcome::NoSection {
        section: Section::Callers,
        kind: CardKind::Diff,
    };
    assert_eq!(traced.cards[0].outcome, expected);
}

#[test]
fn a_diff_beside_a_symbol_target_answers_both_and_a_repeat_shares_its_card() {
    let fixture = Fixture::new();
    let targets = ["diff:base", "load", "diff:base"];
    let traced = fixture.traced(&targets, None, None);
    let traced = traced.unwrap_or_else(|error| panic!("{error}"));
    let kinds: Vec<Option<CardKind>> = (traced.cards.iter())
        .map(|card| match &card.outcome {
            Outcome::Card(card) => Some(card.kind()),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, [Some(CardKind::Diff), Some(CardKind::Symbol), None]);
    assert_eq!(traced.cards[2].outcome, Outcome::Same(0));
}

#[test]
fn a_rev_beside_a_diff_target_an_empty_revision_and_an_unknown_one_are_refused() {
    let fixture = Fixture::new();
    let with_rev = "rev does not apply to a diff target. diff:base compares base with the \
                    working tree.";
    assert_eq!(fixture.refusal("diff:base", Some("base")), with_rev);
    let empty = "a diff target names a revision after diff:, such as diff:main.";
    assert_eq!(fixture.refusal("diff:", None), empty);
    let unknown = fixture.refusal("diff:nope", None);
    assert!(unknown.starts_with("rev nope names no commit"), "{unknown}");
}

#[test]
fn a_diff_with_no_change_lists_no_file() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &BASE);
    let fixture = Fixture::of(sandbox, &root);
    let card = fixture.diff("diff:HEAD", None);
    assert_eq!((card.commits, card.files.total), (0, 0));
    let symbols = card.symbols.unwrap_or_else(|| panic!("the sections"));
    assert_eq!(symbols.changed.total + symbols.moved.total, 0);
}
