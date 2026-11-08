use app::search::{FileRow, Measure};
use app::trace::{
    Callers, CandidatesTrace, Card, Change, FileTrace, FolderEntry, FolderTrace, Linked, Outcome,
    Section, Similar, SymbolTrace, Tally, Trace, TraceRequest, Traced,
};
use app::{Coverage, CoverageStatus, RefGroup, RefSite, SymbolCard};

use crate::{TraceAnswer, trace_text};

fn card(stable_id: &str) -> SymbolCard {
    SymbolCard {
        stable_id: stable_id.to_owned(),
        path: "a.py".to_owned(),
        start_line: 3,
        end_line: 9,
        kind: "function",
        language: "python",
        signature: "def run()".to_owned(),
        parent_class: String::new(),
        body: String::new(),
        body_shown: 0,
        body_total: 0,
    }
}

// The text of one card for `more`, with the card's own target.
fn text_of(card: Card, more: Option<Section>) -> String {
    let request = TraceRequest {
        targets: vec!["run".to_owned()],
        more,
    };
    let trace = Trace {
        cards: vec![Traced {
            target: "run".to_owned(),
            outcome: Outcome::Card(card),
        }],
    };
    trace_text(&TraceAnswer {
        request: &request,
        trace: &trace,
    })
}

fn symbol(edit: impl FnOnce(&mut SymbolTrace)) -> Card {
    let mut traced = SymbolTrace {
        card: card("py:a.py::run"),
        callers: None,
        callees: None,
        tests: None,
        similar: None,
        change: None,
        history: None,
        body: None,
    };
    edit(&mut traced);
    Card::Symbol(Box::new(traced))
}

fn group(path: &str) -> RefGroup {
    RefGroup {
        path: path.to_owned(),
        sites: vec![RefSite {
            line: 4,
            enclosing: "main".to_owned(),
            stable_id: None,
        }],
        tier: "imported",
        tier_label: "resolved-via-import",
        scope_checked: true,
    }
}

fn callers(name_only_files: Tally, rows_total: Tally, status: CoverageStatus) -> Callers {
    Callers {
        bound: 5,
        name_only: 12,
        name_only_files,
        files: 9,
        tiers: None,
        rows: vec![group("b.py"), group("c.py"), group("d.py")],
        rows_total,
        coverage: Coverage {
            status,
            skipped_code_files: 2,
        },
    }
}

#[test]
fn a_cut_section_counts_what_it_hides_and_points_at_more() {
    let complete = CoverageStatus::Complete;
    let exact = callers(Tally::Exact(4), Tally::Exact(5), complete);
    let text = text_of(symbol(|traced| traced.callers = Some(exact)), None);
    assert!(
        text.contains(
            "callers: 5 bound; 12 name-only in 4 files hidden  (more=callers)\n  b.py  \
             imported  main @4\n"
        ),
        "{text}"
    );
    assert!(
        text.ends_with("  d.py  imported  main @4\n  ... 2 more\n"),
        "{text}"
    );
    let floor = callers(Tally::Floor(4), Tally::Floor(5), complete);
    let text = text_of(symbol(|traced| traced.callers = Some(floor)), None);
    assert!(
        text.contains("12 name-only in at least 4 files hidden"),
        "{text}"
    );
    assert!(text.ends_with("  ... at least 2 more\n"), "{text}");
}

// The section in full lists its name-only rows, so its head hides none; the 3 rows hold 3 of the
// 17 sites, so the listing stopped at its limit.
#[test]
fn the_section_that_more_names_hides_nothing_and_counts_what_its_listing_left_out() {
    let full = callers(Tally::Exact(4), Tally::Exact(3), CoverageStatus::Complete);
    let text = text_of(
        symbol(|traced| traced.callers = Some(full)),
        Some(Section::Callers),
    );
    assert!(text.contains("callers: 5 bound\n"), "{text}");
    assert!(
        text.ends_with("  ... 14 more references not listed (limit 500)\n"),
        "{text}"
    );
    assert!(!text.contains("(more="), "{text}");
}

#[test]
fn a_partial_fan_in_says_that_a_caller_can_be_missing() {
    let partial = callers(Tally::Exact(4), Tally::Exact(3), CoverageStatus::Partial);
    let text = text_of(symbol(|traced| traced.callers = Some(partial)), None);
    assert!(
        text.ends_with("not resolved: the scan skipped 2 code files, so a caller can be missing\n"),
        "{text}"
    );
}

#[test]
fn a_last_change_that_git_did_not_give_says_why() {
    let late = Change::NotRead { seconds: 2 };
    let text = text_of(symbol(|traced| traced.change = Some(late)), None);
    assert!(
        text.ends_with("last change: not read in 2 s  (more=history)\n"),
        "{text}"
    );
    let none = Change::Unavailable("history needs git: no head".to_owned());
    let text = text_of(symbol(|traced| traced.change = Some(none)), None);
    assert!(
        text.ends_with("last change: not read: history needs git: no head\n"),
        "{text}"
    );
}

#[test]
fn similar_with_no_bound_ranking_is_its_count() {
    let similar = Similar {
        total: 321,
        rows: Vec::new(),
        bound_total: Tally::Exact(0),
    };
    let text = text_of(symbol(|traced| traced.similar = Some(similar)), None);
    assert!(
        text.ends_with(
            "similar: 321 candidates, ranked by name-only callers; hidden  (more=similar)\n"
        ),
        "{text}"
    );
}

#[test]
fn a_cut_candidates_card_counts_the_rest_and_points_at_more() {
    let candidates = CandidatesTrace {
        total: 14,
        rows: vec![card("py:a.py::run"), card("py:b.py::run#2")],
    };
    let text = text_of(Card::Candidates(candidates), None);
    let expected = "run: 14 definitions. Trace one by its id.\n  [py:a.py::run] @3-9  function\n  \
                    [py:b.py::run#2] @3-9  function\n  ... 12 more  (more=candidates)\n";
    assert_eq!(text, expected);
}

fn file(edit: impl FnOnce(&mut FileTrace)) -> Card {
    let mut traced = FileTrace {
        path: "pkg/__init__.py".to_owned(),
        language: "python",
        lines: 2,
        id_prefix: "py",
        no_symbol_at: None,
        declares: None,
        imports: None,
        importers: None,
        tests: None,
        cycles: None,
        change: None,
    };
    edit(&mut traced);
    Card::File(Box::new(traced))
}

#[test]
fn a_file_with_no_anchor_says_its_imports_were_not_read() {
    let file = file(|traced| traced.imports = Some(Linked::NoAnchor));
    let text = text_of(file, Some(Section::Imports));
    assert!(
        text.ends_with("imports: not read: no symbol of this file joins the name graph\n"),
        "{text}"
    );
}

// A file card has no history section, so no line points at one.
#[test]
fn a_file_card_says_why_git_gave_no_last_change() {
    let cases = [
        (
            Change::NoCommit,
            "last change: not read: unsatisfiable: pkg/__init__.py is not in HEAD (untracked, \
             ignored, or only staged). Commit it, or ask about a tracked file.\n",
        ),
        (
            Change::NotRepository {
                note: "/r is not inside a git repository".to_owned(),
            },
            "last change: not read: history needs git: /r is not inside a git repository\n",
        ),
        (
            Change::Failed("git log exited 128: fatal: bad".to_owned()),
            "last change: not read: git could not answer: git log exited 128: fatal: bad\n",
        ),
        (
            Change::NotRead { seconds: 2 },
            "last change: not read in 2 s\n",
        ),
    ];
    for (change, expected) in cases {
        let text = text_of(file(|traced| traced.change = Some(change)), None);
        assert!(text.ends_with(expected), "{text}");
    }
}

#[test]
fn a_cut_folder_card_points_at_more_and_a_cut_tree_at_a_subfolder() {
    let entry = FolderEntry::File {
        depth: 1,
        row: FileRow {
            path: "src/a.py".to_owned(),
            measure: Measure::Unread("unreadable: permission denied".to_owned()),
        },
    };
    let folder = |depth| FolderTrace {
        path: "src".to_owned(),
        files: 6,
        lines: 1,
        entries: vec![entry.clone()],
        total: 6,
        depth,
    };
    let text = text_of(Card::Folder(folder(1)), None);
    let expected = "src/  6 files, 1 line\n  a.py  not read: unreadable: permission denied\n  ... \
                    5 more  (more=tree)\n";
    assert_eq!(text, expected);
    let text = text_of(Card::Folder(folder(4)), Some(Section::Tree));
    assert!(
        text.ends_with("  ... 5 more: trace a subfolder for the rest\n"),
        "{text}"
    );
}
