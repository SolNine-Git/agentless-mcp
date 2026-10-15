use super::{ParsedFiles, resolve_symbol_span, resolve_symbol_span_in};
use crate::endpoints::scanned::{LINKED, Scanned};

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
