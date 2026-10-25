use facts::{Symbol, SymbolKind};
use syntax::Language;
use text::lower;

use super::{
    DataEntry, FindRequest, FoldedFile, match_rank, matched, matches, short_ascii_holds, top_cards,
    unparsed_mentions,
};
use crate::clock::{assert_spent, spent};
use crate::endpoints::scanned::{Scanned, TESTED};
use crate::scan::{FileSymbols, SymbolFacts};
use crate::{IntArg, symbol_card};

fn data_file(path: &str, names: &[&str]) -> FileSymbols {
    let symbols = names
        .iter()
        .map(|name| symbol(name, "", SymbolKind::Constant));
    FileSymbols {
        path: path.to_owned(),
        language: Language::Json,
        symbols: symbols.collect(),
    }
}

fn found(files: Vec<FileSymbols>, query: &str, limit: usize) -> super::FindResult {
    let scan = SymbolFacts {
        files,
        skipped: Vec::new(),
    };
    let request = FindRequest {
        query: query.to_owned(),
        kind: None,
        limit: IntArg::from_usize(limit),
    };
    matched(&scan, &request, limit)
}

fn folded(path: &str, keys: usize) -> FoldedFile {
    let path = path.to_owned();
    FoldedFile { path, keys }
}

#[test]
fn every_code_match_ranks_before_an_exact_data_key() {
    let code = file("a.py", vec![symbol("area_total", "", SymbolKind::Function)]);
    let result = found(vec![data_file("d.json", &["area"]), code], "area", 20);
    let names: Vec<&str> = result
        .cards
        .rows()
        .iter()
        .map(|card| card.stable_id.as_str())
        .collect();
    assert_eq!(names, ["py:a.py::area_total"]);
    let [DataEntry::Card(card)] = result.data.entries.as_slice() else {
        panic!("one data card: {:?}", result.data.entries);
    };
    assert_eq!(card.stable_id, "json:d.json::area");
}

#[test]
fn a_data_file_with_more_than_three_matches_folds_to_one_entry() {
    let names = ["area", "area_a", "area_b", "area_c"];
    let result = found(vec![data_file("d.json", &names)], "area", 20);
    assert_eq!(
        result.data.entries,
        [DataEntry::Folded(folded("d.json", 4))]
    );
    assert_eq!(result.data.total(), 4);
}

#[test]
fn the_data_part_shows_five_entries_and_lists_the_other_files_by_count() {
    let files: Vec<FileSymbols> = (0..7)
        .map(|at| data_file(&format!("lang/{at}.json"), &["orphan"]))
        .collect();
    let result = found(files, "orphan", 20);
    assert_eq!(result.data.entries.len(), 5);
    assert_eq!(
        result.data.unlisted,
        [folded("lang/5.json", 1), folded("lang/6.json", 1)]
    );
}

#[test]
fn the_code_cards_leave_the_data_part_what_the_limit_has_left() {
    let code = file("a.py", vec![symbol("pin", "", SymbolKind::Function)]);
    let files = vec![
        code,
        data_file("a.json", &["pin"]),
        data_file("b.json", &["pin"]),
    ];
    let result = found(files, "pin", 2);
    assert_eq!(
        (result.cards.rows().len(), result.data.entries.len()),
        (1, 1)
    );
    assert_eq!(result.data.unlisted, [folded("b.json", 1)]);
}

#[test]
fn an_answer_of_one_folded_file_is_not_a_miss() {
    let names: Vec<String> = (0..11).map(|at| format!("darkmode{at}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let result = found(vec![data_file("configdata.json", &names)], "darkmode", 20);
    assert!(result.cards.rows().is_empty());
    assert!(!result.is_miss(), "{:?}", result.data);
    let miss = found(vec![data_file("configdata.json", &names)], "nothing", 20);
    assert!(miss.is_miss());
}

fn symbol(name: &str, parent: &str, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.to_owned(),
        kind,
        line: 1,
        end_line: 1,
        signature: String::new(),
        parent: parent.to_owned(),
        decorated: false,
        bases: Vec::new(),
        is_public: true,
        rationales: Vec::new(),
        duplicate_index: 0,
    }
}

#[test]
fn a_match_holds_the_query_in_its_name_or_qualified_name_and_has_the_kind() {
    let method = symbol("Area", "Square", SymbolKind::Method);
    assert!(matches(&method, "area", None));
    assert!(matches(&method, "square.ar", None));
    assert!(matches(&method, "", Some(SymbolKind::Method)));
    assert!(!matches(&method, "area", Some(SymbolKind::Function)));
    assert!(!matches(&method, "circle", None));
}

#[test]
fn every_name_matches_as_its_two_lowered_tests_do() {
    let long = "Long_".repeat(30);
    let names = [
        ("Area", "Square"),
        ("area", ""),
        ("MiXed_Case", "Outer.Inner"),
        ("", ""),
        (long.as_str(), ""),
        (long.as_str(), "Parent"),
        ("\u{391}\u{3a3}", "a"),
        ("\u{3a3}", "a"),
        ("\u{130}stanbul", ""),
        ("\u{3a9}mega", "Unit"),
    ];
    let queries = [
        "",
        "a",
        "AREA",
        "square.ar",
        ".",
        "e.a",
        "r.mixed",
        "long_l",
        "\u{3c3}",
        "\u{3c2}",
        "a.",
        "i\u{307}",
        "\u{3c9}mega",
        "unit.\u{3a9}",
        "q",
    ];
    for (name, parent) in names {
        let candidate = symbol(name, parent, SymbolKind::Function);
        for query in queries {
            let needle = lower(query);
            let reference = lower(&candidate.name).contains(&needle)
                || lower(&candidate.qualname()).contains(&needle);
            assert_eq!(
                matches(&candidate, &needle, None),
                reference,
                "{parent:?} {name:?} {query:?}"
            );
        }
    }
}

#[test]
fn only_a_short_ascii_name_takes_the_stack_path() {
    let needle = "a";
    let short = symbol("Area", "Square", SymbolKind::Method);
    let long = symbol(&"x".repeat(122), "Parent", SymbolKind::Method);
    let longest = symbol(&"x".repeat(121), "Parent", SymbolKind::Method);
    let greek = symbol("\u{3a3}", "a", SymbolKind::Method);
    assert_eq!(short_ascii_holds(&short, needle), Some(true));
    assert_eq!(short_ascii_holds(&longest, needle), Some(true));
    assert_eq!(short_ascii_holds(&long, needle), None);
    assert_eq!(short_ascii_holds(&greek, needle), None);
}

#[test]
fn an_exact_name_ranks_above_a_qualified_name_above_a_prefix_above_the_rest() {
    let ranks = [
        match_rank(&symbol("Area", "", SymbolKind::Function), "area").0,
        match_rank(&symbol("area", "Square", SymbolKind::Method), "square.area").0,
        match_rank(&symbol("Area_of", "", SymbolKind::Function), "area").0,
        match_rank(&symbol("square_area", "", SymbolKind::Function), "area").0,
    ];
    assert_eq!(ranks, [0, 1, 2, 3]);
    let (_, lowered) = match_rank(&symbol("MiXed", "", SymbolKind::Function), "x");
    assert_eq!(lowered, "mixed");
}

fn file(path: &str, symbols: Vec<Symbol>) -> FileSymbols {
    FileSymbols {
        path: path.to_owned(),
        language: Language::Python,
        symbols,
    }
}

#[test]
fn the_kept_cards_are_the_first_of_the_stable_sort_at_every_limit() {
    let method = |name: &str, parent: &str| symbol(name, parent, SymbolKind::Method);
    let files = [
        file(
            "b.py",
            vec![method("Area", "Square"), method("area", "Circle")],
        ),
        file(
            "a.py",
            vec![method("area", "Square"), method("area_of", "")],
        ),
        file(
            "c.py",
            vec![method("square_area", ""), method("AREA", "Box")],
        ),
        file("a.py", vec![method("area", "Disc"), method("Area", "Ring")]),
    ];
    let needle = "area";
    let all: Vec<(&FileSymbols, &Symbol)> = (files.iter())
        .flat_map(|file| file.symbols.iter().map(move |symbol| (file, symbol)))
        .collect();
    let mut sorted = all.clone();
    sorted.sort_by_cached_key(|&(file, symbol)| {
        let (rank, lowered) = match_rank(symbol, needle);
        (rank, lowered, file.path.as_str())
    });
    for limit in 1..=all.len() + 1 {
        let expected: Vec<_> = (sorted.iter().take(limit))
            .map(|(file, symbol)| symbol_card(file.language, &file.path, symbol, String::new()))
            .collect();
        assert_eq!(
            top_cards(all.clone(), needle, limit),
            expected,
            "limit {limit}"
        );
    }
}

// The check comes before the query test, so an empty query stops too.
#[test]
fn the_unparsed_mention_pass_stops_under_a_spent_deadline() {
    let scanned = Scanned::new(&TESTED);
    let loaded = SymbolFacts::load(&scanned.services, &scanned.reading());
    let scan = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let deadline = spent();
    let reading = scanned.reading().with_deadline(&deadline);
    for query in ["helper", ""] {
        let found = unparsed_mentions(&reading, &scan, query);

        let error = found.value.expect_err("the deadline stops the pass");
        assert_spent(&error);
    }
}
