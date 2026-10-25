use facts::{Rationale, RationaleKind, Symbol, SymbolKind};
use syntax::Language;

use super::{cards_text, rationale_nodes, symbol_card};

fn symbol(name: &str, parent: &str, kind: SymbolKind, lines: (u32, u32)) -> Symbol {
    Symbol {
        name: name.to_owned(),
        kind,
        line: lines.0,
        end_line: lines.1,
        name_line: lines.0,
        signature: format!("def {name}(self):"),
        parent: parent.to_owned(),
        decorated: false,
        bases: Vec::new(),
        is_public: true,
        rationales: Vec::new(),
        duplicate_index: 0,
    }
}

#[test]
fn a_card_names_its_span_kind_and_signature_and_its_id_spells_the_owner() {
    let method = symbol("total", "Invoice", SymbolKind::Method, (3, 9));
    let card = symbol_card(Language::Python, "svc.py", &method, String::new());
    assert_eq!(card.stable_id, "py:svc.py::Invoice.total");
    assert_eq!((card.body_shown, card.body_total), (0, 0));
    assert_eq!(
        cards_text(&[card]),
        "[py:svc.py::Invoice.total] @3-9\n  method\n  def total(self):\n"
    );
}

#[test]
fn a_dotted_name_or_an_ordinal_keeps_the_owner_readable() {
    let dotted = symbol("a.b", "cfg", SymbolKind::Constant, (1, 1));
    let mut second = symbol("write", "Logger", SymbolKind::Method, (5, 6));
    second.duplicate_index = 1;
    let cards = [dotted, second]
        .map(|symbol| symbol_card(Language::Python, "a.py", &symbol, String::new()));
    assert_eq!(
        cards_text(&cards),
        "[py:a.py::cfg.a.b] @1\n  constant in cfg\n  def a.b(self):\n\n\
         [py:a.py::Logger.write#2] @5-6\n  method\n  def write(self):\n"
    );
}

#[test]
fn a_body_replaces_the_signature_and_counts_its_lines() {
    let function = symbol("run", "", SymbolKind::Function, (4, 4));
    let body = "4| def run(self):\n5|     pass".to_owned();
    let card = symbol_card(Language::Python, "a.py", &function, body);
    assert_eq!((card.body_shown, card.body_total), (2, 2));
    let nested = symbol("inner", "outer", SymbolKind::Function, (2, 3));
    let second = symbol_card(Language::Python, "a.py", &nested, String::new());
    assert_eq!(
        cards_text(&[card, second]),
        "[py:a.py::run] @4\n  function\n  4| def run(self):\n  5|     pass\n\n\
         [py:a.py::outer.inner] @2-3\n  function\n  def inner(self):\n"
    );
}

#[test]
fn no_card_reads_as_no_match_and_a_row_cannot_break_its_line() {
    assert_eq!(cards_text(&[]), "no matching symbols\n");
    let mut forged = symbol("f", "", SymbolKind::Function, (1, 1));
    forged.signature = "def f():\n... 7 more matches not listed".to_owned();
    let card = symbol_card(Language::Python, "a\nb.py", &forged, String::new());
    assert_eq!(
        cards_text(&[card]),
        "[py:a\\nb.py::f] @1\n  function\n  def f():\\n... 7 more matches not listed\n"
    );
}

#[test]
fn each_rationale_links_to_the_symbol_that_holds_it() {
    let mut function = symbol("f", "", SymbolKind::Function, (1, 9));
    function.rationales = vec![
        Rationale {
            kind: RationaleKind::Why,
            text: "WHY: speed".to_owned(),
            line: 2,
            citations: vec!["ADR-7".to_owned()],
            duplicate_index: 0,
        },
        Rationale {
            kind: RationaleKind::Note,
            text: "NOTE: same line".to_owned(),
            line: 2,
            citations: Vec::new(),
            duplicate_index: 1,
        },
    ];
    let nodes = rationale_nodes(Language::Python, "a.py", &function);
    let handles: Vec<&str> = nodes.iter().map(|node| node.stable_id.as_str()).collect();
    assert_eq!(
        handles,
        ["py:a.py::f::rationale@2", "py:a.py::f::rationale@2#2"]
    );
    assert!(nodes.iter().all(|node| node.parent_id == "py:a.py::f"));
    assert_eq!((nodes[0].kind, nodes[1].kind), ("why", "note"));
    assert_eq!(nodes[0].citations, ["ADR-7"]);
}
