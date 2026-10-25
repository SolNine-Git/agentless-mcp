use app::{DataEntry, DataMatches, FindResult, FoldedFile, OutsideOutline, Skipped, symbol_card};
use facts::{Symbol, SymbolKind};
use kernel::Bounded;
use syntax::Language;

use super::{FindJson, find_text};
use crate::skipped::skipped_line;

fn card(path: &str, name: &str) -> app::SymbolCard {
    let symbol = Symbol {
        name: name.to_owned(),
        kind: SymbolKind::Constant,
        line: 2,
        end_line: 2,
        signature: format!("{name} = 1"),
        parent: String::new(),
        decorated: false,
        bases: Vec::new(),
        is_public: true,
        rationales: Vec::new(),
        duplicate_index: 0,
    };
    symbol_card(Language::Json, path, &symbol, String::new())
}

fn folded(path: &str, keys: usize) -> FoldedFile {
    let path = path.to_owned();
    FoldedFile { path, keys }
}

fn result(data: DataMatches) -> FindResult {
    FindResult {
        query: "pin".to_owned(),
        cards: Bounded::with_total(Vec::new(), 0, 20),
        data,
        data_kind_excluded: 0,
        outside: OutsideOutline::default(),
        skipped: Vec::new(),
        kind: None,
        other_kinds: Vec::new(),
        unparsed: Vec::new(),
    }
}

#[test]
fn the_data_entries_follow_as_cards_and_folded_lines_then_the_unlisted_files() {
    let data = DataMatches {
        entries: vec![
            DataEntry::Card(card("a.json", "pin")),
            DataEntry::Folded(folded("b.json", 4)),
        ],
        unlisted: (1..=4)
            .map(|at| folded(&format!("l{at}.json"), at))
            .collect(),
        room: 2,
    };
    let expected = "[json:a.json::pin] @2\n  key\n  pin = 1\n\n\
        b.json: 4 data keys match. Grep finds the spellings.\n\
        10 data keys in 4 files match and are not listed: l1.json (1), l2.json (2), l3.json (3), \
        .... This answer has room for 2 data entries after the code matches. Grep finds the \
        spellings.\n";
    let found = result(data);
    assert_eq!(find_text(&found), expected);
    let json = FindJson::of(&found);
    let folded: Vec<(&str, usize)> = json
        .folded
        .iter()
        .map(|file| (file.path, file.keys))
        .collect();
    assert_eq!(json.matches.len(), 1);
    assert_eq!(folded[..2], [("b.json", 4), ("l1.json", 1)]);
    assert_eq!((json.total, json.omitted), (15, 14));
}

#[test]
fn the_fold_beside_an_exact_card_counts_the_other_keys_as_more() {
    let data = DataMatches {
        entries: vec![
            DataEntry::Card(card("package.json", "devDependencies")),
            DataEntry::FoldedRest(folded("package.json", 5)),
        ],
        unlisted: Vec::new(),
        room: 5,
    };
    let expected = "[json:package.json::devDependencies] @2\n  key\n  devDependencies = 1\n\n\
        package.json: 5 more data keys match. Grep finds the spellings.\n";
    let found = result(data);
    assert_eq!(find_text(&found), expected);
    let json = FindJson::of(&found);
    assert_eq!(
        (json.matches.len(), json.folded.len(), json.total),
        (1, 1, 6)
    );
}

#[test]
fn a_kind_filter_that_left_data_keys_out_says_how_many_match() {
    let found = FindResult {
        kind: Some("class"),
        data_kind_excluded: 3,
        ..result(DataMatches::default())
    };
    let expected = "no matching symbols: no declared symbol has 'pin' in its name or qualified \
        name, matched case-insensitively. A parameter, a local variable, an attribute or a string \
        literal is not a declared symbol; grep for the literal to find those.\n\
        3 data keys match, and a kind filter leaves data keys out. Retry with no kind to list \
        them.\n";
    assert_eq!(find_text(&found), expected);
    let json = serde_json::to_value(FindJson::of(&found)).unwrap_or_default();
    assert_eq!(json["data_kind_excluded"], 3);
}

#[test]
fn a_miss_names_five_data_files_outside_their_outline_and_the_files_the_budget_left() {
    let places = (1..=6).map(|at| (format!("s/{at}.json"), at * 100));
    let found = FindResult {
        outside: OutsideOutline {
            places: places.collect(),
            unread: 2,
            budget: 1024,
        },
        ..result(DataMatches::default())
    };
    let text = find_text(&found);
    let tail: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(
        tail,
        [
            "'pin' occurs in 6 data files outside their outline: s/1.json @100, s/2.json @200, \
             s/3.json @300, s/4.json @400, s/5.json @500, ... 1 more. Grep finds every spelling.",
            "note: the text search of data files stops at 1024 bytes, so it did not read 2 data \
             files. Grep reads every file."
        ]
    );
    let json = serde_json::to_value(FindJson::of(&found)).unwrap_or_default();
    let outside = &json["outside_outline"];
    assert_eq!(
        (
            outside["places"][5]["line"].as_u64(),
            outside["unread"].as_u64()
        ),
        (Some(600), Some(2))
    );
    let plain = serde_json::to_value(FindJson::of(&result(DataMatches::default())));
    let plain = plain.unwrap_or_default();
    assert!(plain.get("outside_outline").is_none() && plain.get("data_kind_excluded").is_none());
}

#[test]
fn the_skipped_warning_names_code_files_only() {
    let skipped = |path: &str| Skipped {
        path: path.to_owned(),
        reason: "skipped: too big".to_owned(),
    };
    assert_eq!(skipped_line(&[skipped("package-lock.json")]), "");
    let line = skipped_line(&[skipped("seeded.json"), skipped("huge.py")]);
    assert!(
        line.contains("1 files were skipped") && line.contains("huge.py"),
        "{line}"
    );
    assert!(!line.contains("seeded.json"), "{line}");
}
