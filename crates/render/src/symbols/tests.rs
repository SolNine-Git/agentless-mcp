use app::{
    DataEntry, DataMatches, FindResult, FoldedFile, OutsideOutline, Skipped, Usage, UsageFile,
    UsageOrder, symbol_card,
};
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
        name_line: 2,
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
        usage: None,
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
        ... 1 more. This answer has room for 2 data entries after the code matches. Grep finds \
        the spellings.\n";
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
fn three_unlisted_data_files_are_all_named_with_no_count_of_more() {
    let data = DataMatches {
        entries: vec![DataEntry::Card(card("a.json", "pin"))],
        unlisted: (1..=3)
            .map(|at| folded(&format!("l{at}.json"), at))
            .collect(),
        room: 1,
    };
    let expected = "6 data keys in 3 files match and are not listed: l1.json (1), l2.json (2), \
        l3.json (3). This answer has room for 1 data entry after the code matches. Grep finds \
        the spellings.";
    let text = find_text(&result(data));
    assert_eq!(text.lines().last(), Some(expected));
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
        line.contains("1 file was skipped") && line.contains("huge.py"),
        "{line}"
    );
    assert!(!line.contains("seeded.json"), "{line}");
}

fn usage(
    owner: Option<&str>,
    order: UsageOrder,
    files: &[(&str, u32, u32)],
    total: usize,
) -> Usage {
    let rows = files.iter().map(|(path, line, lines)| UsageFile {
        path: (*path).to_owned(),
        line: *line,
        lines: *lines,
    });
    Usage {
        name: "pin".to_owned(),
        owner: owner.map(str::to_owned),
        order,
        files: Bounded::with_total(rows.collect(), total, 10),
    }
}

#[test]
fn a_miss_with_one_usage_file_says_no_symbol_is_declared_and_lists_the_file() {
    let found = FindResult {
        usage: Some(usage(None, UsageOrder::MostLines, &[("a.py", 3, 1)], 1)),
        ..result(DataMatches::default())
    };
    let expected = "no declared symbol has 'pin' in its name or qualified name, matched \
        case-insensitively.\n\n\
        usage: 1 file spells 'pin' and does not declare it:\n  a.py @3 (1 line)\n";
    assert_eq!(find_text(&found), expected);
}

#[test]
fn the_usage_block_follows_the_matches_after_one_blank_line() {
    let data = DataMatches {
        entries: vec![DataEntry::Card(card("a.json", "pin"))],
        unlisted: Vec::new(),
        room: 5,
    };
    let rows = [("b.py", 1, 3), ("c.py", 2, 1)];
    let found = FindResult {
        usage: Some(usage(Some("Box"), UsageOrder::MostLines, &rows, 2)),
        ..result(data)
    };
    let expected = "[json:a.json::pin] @2\n  key\n  pin = 1\n\n\
        usage: 2 files spell 'pin' and 'Box' and do not declare 'pin'. Most lines first:\n  \
        b.py @1 (3 lines)\n  c.py @2 (1 line)\n";
    assert_eq!(find_text(&found), expected);
}

#[test]
fn a_cut_usage_list_names_its_order_and_counts_the_files_it_leaves_out() {
    let names: Vec<String> = (0..10).map(|at| format!("m{at}.py")).collect();
    let rows: Vec<(&str, u32, u32)> = names.iter().map(|name| (name.as_str(), 1, 2)).collect();
    let found = FindResult {
        usage: Some(usage(None, UsageOrder::NearestDeclaration, &rows, 12)),
        ..result(DataMatches::default())
    };
    let text = find_text(&found);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[2],
        "usage: 12 files spell 'pin' and do not declare it. The 10 nearest to a declaration:"
    );
    assert_eq!(
        (lines[3], lines[12]),
        ("  m0.py @1 (2 lines)", "  m9.py @1 (2 lines)")
    );
    assert_eq!(
        lines[13..],
        ["2 more files spell 'pin' and are not listed."]
    );
    let json = serde_json::to_value(FindJson::of(&found)).unwrap_or_default();
    let member = &json["usage"];
    assert_eq!(
        (&member["name"], &member["total"], &member["limit"]),
        (&"pin".into(), &12.into(), &10.into())
    );
    assert_eq!(member["order"], "nearest_declaration");
    assert_eq!(member["files"][9]["path"], "m9.py");
    assert_eq!(
        (&member["files"][9]["line"], &member["files"][9]["lines"]),
        (&1.into(), &2.into())
    );
    assert!(member.get("owner").is_none());
    let plain = serde_json::to_value(FindJson::of(&result(DataMatches::default())));
    assert!(plain.unwrap_or_default().get("usage").is_none());
}
