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
fn an_id_that_names_no_symbol_of_its_file_claims_no_earlier_state() {
    let scanned = Scanned::new(&LINKED);
    let missed = resolve_symbol_span(&scanned.reading(), "py:pkg/money.py::gone").value;
    let Err(missed) = missed else {
        panic!("a guessed id does not resolve");
    };
    assert_eq!(missed.reason(), "pkg/money.py does not define gone");
}

#[test]
fn a_path_qualified_name_resolves_as_its_stable_id_does_and_a_name_stays_refused() {
    let scanned = Scanned::new(&LINKED);
    let reading = scanned.reading();
    let Ok(span) = resolve_symbol_span(&reading, "pkg/money.py::format_money").value else {
        panic!("a path::name resolves");
    };
    assert_eq!(
        span.stable_id().to_string(),
        "py:pkg/money.py::format_money"
    );
    let Err(missed) = resolve_symbol_span(&reading, "format_money").value else {
        panic!("a bare name does not resolve");
    };
    assert!(
        missed
            .reason()
            .starts_with("not a stable id: 'format_money'")
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
