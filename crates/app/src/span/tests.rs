use super::{Missed, ParsedFiles, resolve_symbol_span, resolve_symbol_span_in};
use crate::endpoints::scanned::{LINKED, Scanned};

#[test]
fn a_data_key_outside_the_outline_names_the_outline_and_grep() {
    let scanned = Scanned::new(&[("rows.json", "{\"a\": {\"b\": {\"c\": 1}}}\n")]);
    let reading = scanned.reading();
    let Err(Missed::Reason(reason)) = resolve_symbol_span(&reading, "json:rows.json::a.b.c").value
    else {
        panic!("a key below depth 2 does not resolve");
    };
    assert!(reason.starts_with("rows.json keeps an outline of its keys, and a.b.c is not in it"));
    assert!(
        resolve_symbol_span(&reading, "json:rows.json::a.b")
            .value
            .is_ok()
    );
}

#[test]
fn ids_that_share_parsed_files_resolve_as_each_id_alone_does() {
    let scanned = Scanned::new(&LINKED);
    let reading = scanned.reading();
    let ids = [
        "py:pkg/money.py::format_money",
        "py:pkg/money.py::unused_helper",
        "py:pkg/money.py::gone",
        "rs:pkg/money.py::format_money",
        "py:pkg/missing.py::f",
        "py:pkg/missing.py::g",
        "py:../outside.py::f",
        "not an id",
        "py:pkg/money.py::format_money",
    ];
    let mut files = ParsedFiles::new();
    for raw in ids {
        let shared = resolve_symbol_span_in(&reading, raw, &mut files);
        let alone = resolve_symbol_span(&reading, raw);
        assert_eq!(format!("{shared:?}"), format!("{alone:?}"), "{raw}");
    }
    let mut parsed: Vec<&str> = files.keys().map(String::as_str).collect();
    parsed.sort_unstable();
    assert_eq!(parsed, ["../outside.py", "pkg/missing.py", "pkg/money.py"]);
}
