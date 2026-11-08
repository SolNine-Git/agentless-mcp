use std::path::Path;
use std::time::Duration;

use store::FactSource;

use super::change::{file_change, last_change};
use super::{
    CANDIDATE_ROWS, Card, CardKind, Change, FileTrace, FolderEntry, Linked, Miss, Outcome, Section,
    SymbolTrace, Tally, Trace, TraceRequest, trace,
};
use crate::endpoints::scanned::{LINKED, Scanned, TESTED};
use crate::search::{Measure, Place};
use crate::testing::Sandbox;
use crate::{Reading, Repo};

// The scanned repository read with its git state, which history needs.
fn with_git<T>(scanned: &Scanned, read: impl FnOnce(&Reading<'_>) -> T) -> T {
    let base = scanned.reading();
    let root = base.repo.root.clone();
    let snapshot = scanned.services.git.snapshot(&root, Some(&root));
    read(&Reading {
        snapshot: &snapshot,
        ..base
    })
}

fn traced_in(scanned: &Scanned, targets: &[&str], more: Option<Section>) -> Trace {
    let request = TraceRequest {
        targets: targets.iter().map(|target| (*target).to_owned()).collect(),
        more,
    };
    let outcome = with_git(scanned, |reading| {
        trace(&scanned.services, reading, &request)
    });
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

fn card_in(scanned: &Scanned, target: &str, more: Option<Section>) -> Card {
    let mut traced = traced_in(scanned, &[target], more);
    match traced.cards.remove(0).outcome {
        Outcome::Card(card) => card,
        other => panic!("{target}: a card, got {other:?}"),
    }
}

fn card(files: &[(&str, &str)], target: &str, more: Option<Section>) -> Card {
    card_in(&Scanned::new(files), target, more)
}

fn symbol(card: Card) -> SymbolTrace {
    match card {
        Card::Symbol(traced) => *traced,
        other => panic!("a symbol card, got {other:?}"),
    }
}

fn file(card: Card) -> FileTrace {
    match card {
        Card::File(traced) => *traced,
        other => panic!("a file card, got {other:?}"),
    }
}

fn miss(card: Card) -> Miss {
    match card {
        Card::Miss(miss) => miss,
        other => panic!("a miss, got {other:?}"),
    }
}

// Two definitions of `run`, a constant, a class with a method, and a README no grammar reads.
const MIXED: [(&str, &str); 5] = [
    (
        "pkg/jobs.py",
        "LIMIT = 3\n\n\nclass Job:\n    def run(self):\n        return LIMIT\n\n\n# end\n",
    ),
    ("pkg/other.py", "def run():\n    return 2\n"),
    ("pkg/__init__.py", ""),
    ("README.md", "# jobs\nzz_text_only appears here\n"),
    (".agentless-mcp.json", "{}\n"),
];

#[test]
fn the_resolution_order_reads_an_id_a_line_a_path_a_qualified_name_and_a_bare_name() {
    let scanned = Scanned::new(&MIXED);
    let by_id = symbol(card_in(&scanned, "py:pkg/jobs.py::Job.run", None));
    assert_eq!(by_id.card.stable_id, "py:pkg/jobs.py::Job.run");
    let by_line = symbol(card_in(&scanned, "pkg/jobs.py:6", None));
    assert_eq!(by_line.card.stable_id, "py:pkg/jobs.py::Job.run");
    let by_path = file(card_in(&scanned, "pkg/jobs.py", None));
    assert_eq!((by_path.path.as_str(), by_path.lines), ("pkg/jobs.py", 9));
    let by_folder = card_in(&scanned, "pkg/", None);
    assert_eq!(by_folder.kind(), CardKind::Folder);
    let by_qualified = symbol(card_in(&scanned, "Job.run", None));
    assert_eq!(by_qualified.card.stable_id, "py:pkg/jobs.py::Job.run");
    let Card::Candidates(by_name) = card_in(&scanned, "run", None) else {
        panic!("a candidates card");
    };
    let ids: Vec<&str> = by_name
        .rows
        .iter()
        .map(|row| row.stable_id.as_str())
        .collect();
    assert_eq!(ids, ["py:pkg/other.py::run", "py:pkg/jobs.py::Job.run"]);
    assert_eq!(by_name.total, 2);
}

#[test]
fn a_line_that_no_symbol_spans_gives_the_file_card_and_a_line_past_the_end_says_so() {
    let scanned = Scanned::new(&MIXED);
    let unheld = file(card_in(&scanned, "pkg/jobs.py:9", None));
    assert_eq!(unheld.no_symbol_at, Some(9));
    let past = miss(card_in(&scanned, "pkg/jobs.py:40", None));
    let expected = Miss::LinePastEnd {
        path: "pkg/jobs.py".to_owned(),
        line: 40,
        lines: 9,
    };
    assert_eq!(past, expected);
}

#[test]
fn a_path_reads_from_the_root_as_a_relative_an_absolute_or_a_url_path() {
    let scanned = Scanned::new(&MIXED);
    let root = scanned.reading().repo.root.display().to_string();
    let forms = [
        "./pkg/jobs.py".to_owned(),
        format!("{root}/pkg/jobs.py"),
        "https://example.invalid/o/r/blob/main/pkg/jobs.py#L4".to_owned(),
        format!("File \"{root}/pkg/jobs.py\", line 5, in run"),
    ];
    for form in forms {
        let traced = file(card_in(&scanned, &form, None));
        assert_eq!(traced.path, "pkg/jobs.py", "{form}");
    }
}

#[test]
fn each_file_miss_says_why_the_file_has_no_card() {
    let big = format!("# {}\n", "x".repeat(1_000_001));
    let mut files = MIXED.to_vec();
    files.push(("big.py", big.as_str()));
    let scanned = Scanned::new(&files);
    let no_grammar = miss(card_in(&scanned, "README.md", None));
    assert_eq!(
        no_grammar,
        Miss::NoGrammar {
            path: "README.md".to_owned()
        }
    );
    let config = miss(card_in(&scanned, ".agentless-mcp.json", None));
    assert_eq!(
        config,
        Miss::NotScanned {
            path: ".agentless-mcp.json".to_owned()
        }
    );
    let Miss::NotParsed { path, reason } = miss(card_in(&scanned, "big.py", None)) else {
        panic!("a file the scan passed over");
    };
    assert_eq!(path, "big.py");
    assert!(!reason.is_empty());
    let unknown = miss(card_in(&scanned, "jobs.py", None));
    let expected = Miss::UnknownPath {
        path: "jobs.py".to_owned(),
        endings: vec!["pkg/jobs.py".to_owned()],
    };
    assert_eq!(unknown, expected);
}

#[test]
fn a_name_that_matches_nothing_gives_its_usage_files_or_where_its_text_occurs() {
    let files = [
        ("a.py", "def caller():\n    return missing_name(1)\n"),
        ("notes.txt", "zz_text_only here\n"),
    ];
    let scanned = Scanned::new(&files);
    let Miss::Nothing { usage, literal, .. } = miss(card_in(&scanned, "missing_name", None)) else {
        panic!("a name miss");
    };
    let usage = usage.unwrap_or_else(|| panic!("usage files"));
    assert_eq!(usage.files.total(), 1);
    assert_eq!(usage.files.rows()[0].path, "a.py");
    assert_eq!(literal, None);
    let Miss::Nothing { usage, literal, .. } = miss(card_in(&scanned, "zz_text_only", None)) else {
        panic!("a name miss");
    };
    assert_eq!(usage, None);
    let place = Place {
        path: "notes.txt".to_owned(),
        line: 1,
    };
    assert_eq!(literal, Some((vec![place], 1)));
}

#[test]
fn each_miss_of_a_batch_is_the_miss_that_its_target_gives_alone() {
    let files = [
        (
            "a.py",
            "def caller():\n    return missing_name(other_name)\n",
        ),
        ("b.py", "X = 'zz_in_code'\n"),
        ("notes.txt", "zz_text_only and other_name here\n"),
        ("data.json", "{\"zz_text_only\": 1}\n"),
    ];
    let scanned = Scanned::new(&files);
    let targets = [
        "missing_name",
        "zz_text_only",
        "other_name",
        "zz_text_only",
        "zz in code",
        "zz_in_code",
        "   ",
        "pkg/missing.py",
    ];
    let batch = traced_in(&scanned, &targets, None);
    let outcomes: Vec<&Outcome> = batch.cards.iter().map(|card| &card.outcome).collect();
    assert_eq!(outcomes[3], &Outcome::Same(1));
    for (target, outcome) in targets
        .iter()
        .zip(outcomes)
        .filter(|(_, outcome)| !matches!(outcome, Outcome::Same(_)))
    {
        let alone = traced_in(&scanned, &[target], None);
        assert_eq!(outcome, &alone.cards[0].outcome, "{target}");
    }
    let Miss::Nothing { literal, .. } = miss(card_in(&scanned, "zz_text_only", None)) else {
        panic!("a name miss");
    };
    let (_, files) = literal.unwrap_or_else(|| panic!("the places of the text"));
    assert_eq!(
        files, 2,
        "the text is found in a data file and a file that no grammar reads"
    );
}

#[test]
fn a_target_list_takes_one_to_eight_targets_and_a_repeat_shares_its_first_card() {
    let scanned = Scanned::new(&MIXED);
    let traced = traced_in(&scanned, &["Job", "pkg/other.py", "Job"], None);
    assert_eq!(traced.cards.len(), 3);
    assert_eq!(traced.cards[2].outcome, Outcome::Same(0));
    assert_eq!(traced.cards[2].target, "Job");
    for count in [0, 9] {
        let request = TraceRequest {
            targets: vec!["Job".to_owned(); count],
            more: None,
        };
        let refused = trace(&scanned.services, &scanned.reading(), &request).value;
        let error = refused
            .err()
            .unwrap_or_else(|| panic!("{count} targets are refused"));
        assert_eq!(
            error.to_string(),
            format!("target takes a value from 1 through 8, got {count}")
        );
    }
}

#[test]
fn more_names_a_section_that_the_card_kind_does_not_hold() {
    let scanned = Scanned::new(&MIXED);
    let traced = traced_in(&scanned, &["pkg/jobs.py"], Some(Section::Callers));
    let expected = Outcome::NoSection {
        section: Section::Callers,
        kind: CardKind::File,
    };
    assert_eq!(traced.cards[0].outcome, expected);
    let candidates = card_in(&scanned, "run", Some(Section::Callers));
    assert_eq!(candidates.kind(), CardKind::Candidates);
}

#[test]
fn a_symbol_card_splits_callers_from_tests_and_reads_callees_similar_and_coverage() {
    let traced = symbol(card(&TESTED, "py:core.py::helper", None));
    let callers = traced.callers.unwrap_or_else(|| panic!("callers"));
    let rows: Vec<(&str, &str)> = (callers.rows.iter())
        .map(|group| (group.path.as_str(), group.tier))
        .collect();
    assert_eq!(rows, [("user.py", "imported")]);
    assert_eq!(callers.rows_total, Tally::Exact(1));
    assert_eq!((callers.bound, callers.name_only, callers.files), (4, 0, 2));
    let tests = traced.tests.unwrap_or_else(|| panic!("tests"));
    assert_eq!(tests.rows[0].path, "tests/test_core.py");
    assert_eq!(tests.total, Tally::Exact(1));
    let fan_out = traced.callees.unwrap_or_else(|| panic!("callees"));
    assert!(fan_out.rows.is_empty());
    assert!(traced.similar.is_some());
    assert!(traced.history.is_none() && traced.body.is_none());
    assert!(matches!(
        traced.change,
        Some(Change::Commit(_) | Change::NotRead { .. })
    ));
}

#[test]
fn a_count_comes_from_the_complete_list_when_a_tier_passes_the_largest_listing() {
    let calls = "    helper()\n".repeat(600);
    let many = format!("def caller():\n{calls}");
    let files = [
        ("core.py", "def helper():\n    return 1\n"),
        ("other.py", "def helper():\n    return 2\n"),
        ("many.py", many.as_str()),
    ];
    let traced = symbol(card(&files, "py:core.py::helper", Some(Section::Callers)));
    let callers = traced.callers.unwrap_or_else(|| panic!("callers"));
    assert_eq!(callers.bound + callers.name_only, 600);
    assert!(callers.name_only > crate::ops::limits::MAX_LIMIT);
    assert_eq!(callers.files, 1);
    assert_eq!(callers.tiers, None);
    assert_eq!(callers.name_only_files, Tally::Floor(1));
}

// The bare `run` binds to neither definition, so explain lists an edge to each.
#[test]
fn a_symbol_card_counts_its_bound_and_its_name_only_callees() {
    let files = [
        ("a.py", "def run():\n    return 1\n"),
        ("b.py", "def run():\n    return 2\n"),
        (
            "c.py",
            "from a import run as first\n\n\ndef helper():\n    return 3\n\n\n\
             def go():\n    return first() + helper() + run()\n",
        ),
    ];
    let traced = symbol(card(&files, "py:c.py::go", Some(Section::Callees)));
    let callees = traced.callees.unwrap_or_else(|| panic!("callees"));
    let total: usize = callees.tiers.iter().map(|tier| tier.total).sum();
    assert_eq!((callees.bound, callees.name_only, total), (2, 2, 4));
}

// `a_user.py` calls `helper` by name with no import, so its site is name-only.
#[test]
fn more_lists_every_caller_binding_tiers_first_and_the_default_card_the_bound_ones() {
    let mut files = TESTED.to_vec();
    files.push((
        "a_user.py",
        "def by_name(value):\n    return helper(value)\n",
    ));
    let scanned = Scanned::new(&files);
    let rows = |more| {
        let traced = symbol(card_in(&scanned, "py:core.py::helper", more));
        let callers = traced.callers.unwrap_or_else(|| panic!("callers"));
        let rows = callers
            .rows
            .iter()
            .map(|group| (group.path.clone(), group.tier));
        rows.collect::<Vec<_>>()
    };
    assert_eq!(rows(None), [("user.py".to_owned(), "imported")]);
    let full = [
        ("tests/test_core.py".to_owned(), "imported"),
        ("user.py".to_owned(), "imported"),
        ("a_user.py".to_owned(), "unique"),
    ];
    assert_eq!(rows(Some(Section::Callers)), full);
}

#[test]
fn more_shows_one_section_alone_and_the_body_and_history_in_full() {
    let scanned = Scanned::new(&TESTED);
    let only = symbol(card_in(
        &scanned,
        "py:core.py::helper",
        Some(Section::Callees),
    ));
    assert!(only.callees.is_some());
    assert!(only.callers.is_none() && only.tests.is_none() && only.change.is_none());
    let body = symbol(card_in(&scanned, "py:core.py::helper", Some(Section::Body)));
    let body = body.body.unwrap_or_else(|| panic!("a body"));
    let body = body.unwrap_or_else(|reason| panic!("{reason}"));
    assert!(body.body.contains("return value"), "{}", body.body);
    let history = symbol(card_in(
        &scanned,
        "py:core.py::helper",
        Some(Section::History),
    ));
    let history = history.history.unwrap_or_else(|| panic!("a history"));
    let history = history.unwrap_or_else(|reason| panic!("{reason}"));
    assert_eq!(history.entries[0].subject, "fixture");
}

#[test]
fn the_last_change_is_the_newest_commit_on_the_span() {
    let scanned = Scanned::new(&TESTED);
    let wait = Duration::from_secs(30);
    let change = with_git(&scanned, |reading| {
        last_change(&scanned.services, reading, "py:core.py::helper", wait)
    });
    let Change::Commit(entry) = change else {
        panic!("a commit: {change:?}");
    };
    assert_eq!(entry.subject, "fixture");
}

#[test]
fn a_default_file_card_reads_the_newest_commit_on_the_file_and_more_does_not() {
    let scanned = Scanned::new(&MIXED);
    let traced = file(card_in(&scanned, "pkg/jobs.py", None));
    match &traced.change {
        Some(Change::Commit(entry)) => assert_eq!(entry.subject, "fixture"),
        late => assert!(matches!(late, Some(Change::NotRead { .. })), "{late:?}"),
    }
    let only = file(card_in(&scanned, "pkg/jobs.py", Some(Section::Declares)));
    assert_eq!(only.change, None);
}

#[test]
fn a_file_that_no_commit_touched_and_a_root_outside_git_have_their_own_change() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", "a = 1\n")]);
    sandbox.put("repo/new.py", "b = 2\n");
    let plain = sandbox.mkdir("plain");
    sandbox.put("plain/c.py", "c = 3\n");
    let services = sandbox.services(&root);
    let change_at = |dir: &Path, path: &str| {
        let repo = Repo::load(&services, dir).unwrap_or_else(|error| panic!("{error}"));
        let snapshot = services
            .git
            .snapshot(dir, services.git.root(dir).as_deref());
        let source = FactSource::parsing();
        let reading = Reading {
            repo: &repo,
            snapshot: &snapshot,
            facts: &source,
            deadline: None,
        };
        file_change(&services, &reading, path, Duration::from_secs(30))
    };
    assert_eq!(change_at(&root, "new.py"), Change::NoCommit);
    let outside = change_at(&plain, "c.py");
    let Change::NotRepository { note } = &outside else {
        panic!("no repository: {outside:?}");
    };
    assert!(note.contains("is not inside a git repository"), "{note}");
}

#[test]
fn a_file_card_counts_its_kinds_keeps_constants_last_and_reads_its_imports() {
    let constants = "A = 1\nB = 2\nC = 3\nD = 4\nE = 5\nF = 6\nG = 7\nH = 8\n";
    let money = format!("{constants}\n\ndef format_money(amount):\n    return str(amount)\n");
    let mut files = LINKED.to_vec();
    files[1] = ("pkg/money.py", money.as_str());
    let traced = file(card(&files, "pkg/money.py", None));
    let declares = traced.declares.unwrap_or_else(|| panic!("declarations"));
    assert_eq!(declares.total, 9);
    assert_eq!(declares.kinds, [("constant", 8), ("function", 1)]);
    let kept: Vec<&str> = (declares.rows.iter())
        .map(|row| row.stable_id.as_str())
        .collect();
    assert_eq!(kept.len(), 8);
    assert_eq!(kept[7], "py:pkg/money.py::format_money");
    let Some(Linked::Found(importers)) = traced.importers else {
        panic!("importers");
    };
    assert_eq!(importers.rows[0].path, "pkg/report.py");
    assert_eq!(importers.total, Tally::Exact(1));
    assert_eq!(traced.id_prefix, "py");
}

#[test]
fn a_file_in_an_import_cycle_holds_it() {
    let files = [
        ("pkg/__init__.py", ""),
        (
            "pkg/a.py",
            "from pkg.b import two\n\n\ndef one():\n    return two()\n",
        ),
        (
            "pkg/b.py",
            "from pkg.a import one\n\n\ndef two():\n    return one()\n",
        ),
    ];
    let traced = file(card(&files, "pkg/a.py", Some(Section::Cycles)));
    let cycles = traced.cycles.unwrap_or_else(|| panic!("cycles"));
    assert_eq!(cycles.total, Tally::Exact(1));
    assert_eq!(cycles.rows[0].members, ["pkg/a.py", "pkg/b.py"]);
    assert!(traced.declares.is_none());
}

#[test]
fn a_folder_card_lists_its_entries_to_one_level_with_rows_and_more_goes_deeper() {
    let files = [
        ("src/a.py", "def one():\n    return 1\n"),
        ("src/deep/b.py", "x = 1\n"),
        ("src/deep/c.py", "y = 2\n"),
    ];
    let scanned = Scanned::new(&files);
    let Card::Folder(folder) = card_in(&scanned, "src", None) else {
        panic!("a folder card");
    };
    assert_eq!((folder.files, folder.lines, folder.total), (3, 4, 2));
    let FolderEntry::File { row, depth } = &folder.entries[0] else {
        panic!("a file entry");
    };
    assert_eq!((row.path.as_str(), *depth), ("src/a.py", 1));
    let counted = Measure::Counted {
        lines: 2,
        symbols: Some(1),
    };
    assert_eq!(row.measure, counted);
    let expected = FolderEntry::Folder {
        depth: 1,
        path: "src/deep".to_owned(),
        files: 2,
    };
    assert_eq!(folder.entries[1], expected);
    let Card::Folder(deeper) = card_in(&scanned, "src", Some(Section::Tree)) else {
        panic!("a folder card");
    };
    assert_eq!(deeper.total, 4);
}

#[test]
fn a_candidates_card_cuts_at_its_cap_and_more_lists_every_definition() {
    let paths: Vec<String> = (0..20).map(|at| format!("m{at:02}.py")).collect();
    let files: Vec<(&str, &str)> = (paths.iter())
        .map(|path| (path.as_str(), "def run():\n    return 1\n"))
        .collect();
    let scanned = Scanned::new(&files);
    let Card::Candidates(cut) = card_in(&scanned, "run", None) else {
        panic!("a candidates card");
    };
    assert_eq!((cut.total, cut.rows.len()), (20, CANDIDATE_ROWS));
    let Card::Candidates(all) = card_in(&scanned, "run", Some(Section::Candidates)) else {
        panic!("a candidates card");
    };
    assert_eq!(all.rows.len(), 20);
}

#[test]
fn every_section_reads_back_from_its_name() {
    for section in Section::ALL {
        assert_eq!(Section::from_name(section.as_str()), Some(section));
    }
    assert_eq!(Section::from_name("usage"), None);
}
