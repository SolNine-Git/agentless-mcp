use facts::{Symbol, SymbolKind};

use super::{match_rank, matches};

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
