use facts::{Symbol, SymbolKind};

use super::{first_line_outside, outside_outline, read_within};
use crate::IntArg;
use crate::clock::{assert_spent, spent};
use crate::endpoints::scanned::Scanned;
use crate::find::{FindRequest, FindResult, find};
use crate::scan::SymbolFacts;
use crate::testing::Sandbox;

const TARGET: &str = "Stop live broadcasting?";

// 500 keys fill the outline, so the key on line 502 is past its cap.
fn capped_strings() -> String {
    let keys: Vec<String> = (0..500).map(|at| format!("  \"k{at}\": \"v\",")).collect();
    format!("{{\n{}\n  \"{TARGET}\": \"Stop\"\n}}\n", keys.join("\n"))
}

const DEEP: &str = "{\n  \"a\": {\n    \"b\": {\n      \"zzneedle\": 1\n    }\n  }\n}\n";
const LISTED: &str = "{\n  \"a\": [\n    \"zzneedle\"\n  ]\n}\n";

fn found(files: &[(&str, &str)], query: &str, kind: Option<SymbolKind>) -> FindResult {
    let scanned = Scanned::new(files);
    let request = FindRequest {
        query: query.to_owned(),
        kind,
        limit: IntArg::from_usize(20),
    };
    let answered = find(&scanned.services, &scanned.reading(), &request);
    answered.value.unwrap_or_else(|error| panic!("{error}"))
}

fn places(result: &FindResult) -> Vec<(&str, u32)> {
    let places = result.outside.places.iter();
    places.map(|(path, line)| (path.as_str(), *line)).collect()
}

#[test]
fn a_miss_names_the_key_past_the_outline_cap_with_its_line() {
    let strings = capped_strings();
    let result = found(&[("strings/en.json", &strings)], TARGET, None);
    assert!(result.is_miss());
    assert_eq!(places(&result), [("strings/en.json", 502)]);
    assert_eq!(result.outside.unread, 0);
}

#[test]
fn an_answer_with_only_data_cards_names_the_files_that_hold_the_query_outside_the_outline() {
    let strings = capped_strings();
    let small = format!("{{\n  \"{TARGET}\": \"Stop\"\n}}\n");
    let files = [
        ("strings/de.json", small.as_str()),
        ("strings/en.json", &strings),
    ];
    let result = found(&files, TARGET, None);
    assert!(result.cards.rows().is_empty() && !result.is_miss());
    assert_eq!(places(&result), [("strings/en.json", 502)]);
}

#[test]
fn a_key_below_the_outline_depth_and_a_list_item_count_and_a_row_line_does_not() {
    let row = "{\n  \"a\": \"zzneedle\"\n}\n";
    let files = [
        ("deep.json", DEEP),
        ("listed.json", LISTED),
        ("row.json", row),
    ];
    let result = found(&files, "ZZneedle", None);
    assert_eq!(places(&result), [("deep.json", 4), ("listed.json", 3)]);
}

#[test]
fn a_data_file_the_size_cap_skipped_gets_the_text_search() {
    let big = format!(
        "{{\"pad\": \"{}\",\n\"zzbig\": 1}}\n",
        "x".repeat(1_000_001)
    );
    let result = found(&[("big.json", &big)], "zzbig", None);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(places(&result), [("big.json", 2)]);
}

#[test]
fn a_kind_filter_or_a_code_card_skips_the_search() {
    let code = "def zzneedle():\n    return 1\n";
    let kinded = found(
        &[("deep.json", DEEP)],
        "zzneedle",
        Some(SymbolKind::Function),
    );
    let carded = found(&[("deep.json", DEEP), ("a.py", code)], "zzneedle", None);
    assert_eq!(kinded.outside.places.len() + carded.outside.places.len(), 0);
}

#[test]
fn a_file_larger_than_the_bytes_left_is_not_read() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("ten.json", "[1,2,3,45]")]);
    let path = root.join("ten.json");
    let read = |left| read_within(&path, left).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(read(9), None);
    assert_eq!(read(10).map(|bytes| bytes.len()), Some(10));
}

#[test]
fn the_first_line_outside_skips_the_lines_where_outline_rows_start() {
    let rows = [Symbol {
        name: "a".to_owned(),
        kind: SymbolKind::Constant,
        line: 2,
        end_line: 2,
        signature: String::new(),
        parent: String::new(),
        decorated: false,
        bases: Vec::new(),
        is_public: true,
        rationales: Vec::new(),
        duplicate_index: 0,
    }];
    assert_eq!(first_line_outside("x\nzz\nb ZZ\n", "zz", &rows), Some(3));
    assert_eq!(first_line_outside("x\nzz\n", "zz", &rows), None);
}

#[test]
fn the_search_stops_under_a_spent_deadline() {
    let scanned = Scanned::new(&[("deep.json", DEEP)]);
    let loaded = SymbolFacts::load(&scanned.services, &scanned.reading());
    let scan = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let deadline = spent();
    let reading = scanned.reading().with_deadline(&deadline);
    let searched = outside_outline(&reading, &scan, "zzneedle");
    assert_spent(&searched.value.expect_err("the deadline stops the search"));
}
