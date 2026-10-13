use super::{first_copies, grouped_rows, shorten};
use crate::{SymbolCard, TokenCounter, cards_text};

// One token per character, so the arithmetic of a cut reads off the text.
struct PerChar;

impl TokenCounter for PerChar {
    fn count(&self, text: &str) -> usize {
        text.chars().count()
    }
}

fn card(body: &str) -> SymbolCard {
    let lines = body.split('\n').count();
    SymbolCard {
        stable_id: "py:a.py::f".to_owned(),
        path: "a.py".to_owned(),
        start_line: 1,
        end_line: 3,
        kind: "function",
        language: "python",
        signature: "def f()".to_owned(),
        parent_class: String::new(),
        body: body.to_owned(),
        body_shown: lines,
        body_total: lines,
    }
}

#[test]
fn a_repeated_id_expands_at_its_first_copy() {
    let ids = ["b", "a", "b", "c", "a"].map(str::to_owned);
    assert_eq!(first_copies(&ids), ["b", "a", "c"]);
}

#[test]
fn the_ids_past_the_limit_come_before_the_repeated_ids() {
    let rows = grouped_rows(2, 3, 1);
    let counts: Vec<&str> = rows.iter().map(|(count, _)| count.as_str()).collect();
    assert_eq!(counts, ["(2 ids)", "(3 ids)"]);
    assert!(rows[0].1.ends_with("the per-call limit is 1 symbols"));
    assert!(grouped_rows(0, 0, 9).is_empty());
}

#[test]
fn a_cut_keeps_the_lines_that_fit_and_names_the_cut() {
    let body = "1| aaaa\n2| bbbb\n3| cccc";
    let header = PerChar.count(&cards_text(&[card("")]));
    let cut = shorten(&PerChar, &card(body), header + 32 + 15);
    assert_eq!((cut.body_shown, cut.body_total), (2, 3));
    assert!(cut.body.starts_with("1| aaaa\n2| bbbb\n"), "{}", cut.body);
}

#[test]
fn no_share_keeps_one_line() {
    let cut = shorten(&PerChar, &card("1| aaaa\n2| bbbb"), 0);
    assert_eq!((cut.body_shown, cut.body_total), (1, 2));
    assert!(cut.body.starts_with("1| aaaa\n"), "{}", cut.body);
}
