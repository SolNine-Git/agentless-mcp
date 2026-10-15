use facts::{Symbol, SymbolKind};
use syntax::Language;
use text::lower;

use super::{match_rank, matches, short_ascii_holds, top_cards};
use crate::scan::FileSymbols;
use crate::symbol_card;

fn symbol(name: &str, parent: &str, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.to_owned(),
        kind,
        line: 1,
        end_line: 1,
        signature: String::new(),
        parent: parent.to_owned(),
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
