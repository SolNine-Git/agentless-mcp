use app::{DataEntry, DataMatches, FindResult, FoldedFile, Skipped, symbol_card};
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
        scanned: true,
    };
    let expected = "[json:a.json::pin] @2\n  constant\n  pin = 1\n\n\
        b.json: 4 data keys match. Grep finds the spellings.\n\
        10 data keys in 4 files match and are not listed: l1.json (1), l2.json (2), l3.json (3), \
        .... Grep finds the spellings.\n";
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
fn a_miss_in_a_repository_with_data_files_says_deep_keys_are_not_symbols() {
    let with_data = DataMatches {
        scanned: true,
        ..DataMatches::default()
    };
    let text = find_text(&result(with_data));
    assert!(text.ends_with("not symbols. Grep finds them.\n"), "{text}");
    let text = find_text(&result(DataMatches::default()));
    assert!(!text.contains("not symbols"), "{text}");
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
