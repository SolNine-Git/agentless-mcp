use super::{HistoryEntry, entry, history_entry_text, line_range, shorten};
use crate::TokenCounter;

// One token per character, so the arithmetic of a cut reads off the text.
struct PerChar;

impl TokenCounter for PerChar {
    fn count(&self, text: &str) -> usize {
        text.chars().count()
    }
}

fn commit(body: &str) -> git::Commit {
    git::Commit {
        sha: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        authored: "2026-02-01T00:00:00+00:00".to_owned(),
        subject: "Add the calculator".to_owned(),
        body: body.to_owned(),
    }
}

#[test]
fn a_body_loses_its_blank_edges_and_keeps_its_inner_blank_lines() {
    let entry = entry(commit("\n \nfirst\n\n\tsecond\r\n  \n"));
    assert_eq!(entry.body_lines, ["first", "", "\tsecond"]);
    assert_eq!((entry.body_shown(), entry.body_total), (3, 3));
    assert_eq!(entry.short_sha(), "01234567");
}

#[test]
fn an_entry_row_indents_its_body_and_marks_a_cut() {
    let whole = entry(commit("why\n\nmore"));
    assert_eq!(
        history_entry_text(&whole),
        "  01234567  2026-02-01T00:00:00+00:00  Add the calculator\n    why\n\n    more\n"
    );
    let cut = HistoryEntry {
        body_lines: vec!["why".to_owned()],
        ..whole
    };
    let text = history_entry_text(&cut);
    assert!(text.starts_with("  01234567  "), "{text}");
    assert!(
        text.lines()
            .last()
            .is_some_and(|line| line.starts_with("  ")),
        "{text}"
    );
    assert_eq!(text.lines().count(), 3);
}

#[test]
fn a_cut_keeps_the_rows_that_fit_and_may_keep_none() {
    let long = entry(commit("aaaa\nbbbb\ncccc"));
    let bare = HistoryEntry {
        body_lines: Vec::new(),
        body_total: 0,
        ..long.clone()
    };
    let header = PerChar.count(&history_entry_text(&bare));
    let two_rows = "    aaaa\n    bbbb".chars().count();
    let cut = shorten(&PerChar, long.clone(), header + 32 + two_rows);
    assert_eq!((cut.body_shown(), cut.body_total), (2, 3));
    let none = shorten(&PerChar, long, 0);
    assert_eq!((none.body_shown(), none.body_total), (0, 3));
}

#[test]
fn a_span_that_ends_before_it_starts_is_refused_with_the_bound_text() {
    assert!(line_range(3, 5).is_ok());
    let refused = |start, end| line_range(start, end).err().map(|error| error.to_string());
    assert_eq!(
        refused(0, 5).as_deref(),
        Some("start must be at least 1, got 0")
    );
    assert_eq!(
        refused(5, 3).as_deref(),
        Some("end must be at least 5, got 3")
    );
}
