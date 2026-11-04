use app::RefsRequest;

use super::goldens_read::{Pinned, golden, int};

// The target of each repository's refs golden, as the golden harness names it.
const CASES: [(&str, &str); 3] = [
    ("repo_py", "format_money"),
    ("repo_ts", "formatMoney"),
    ("repo_nested", "format_amount"),
];

#[test]
fn the_refs_goldens_come_out_byte_for_byte() {
    for (repo, target) in CASES {
        let pinned = Pinned::new(repo);
        let request = RefsRequest {
            target: target.to_owned(),
            limit: int("50"),
            shared_callers: false,
        };
        let noted = app::refs(&pinned.services, &pinned.reading(), &request);
        assert!(
            noted.diagnostics.is_empty(),
            "{repo}: {:?}",
            noted.diagnostics
        );
        let result = noted
            .value
            .unwrap_or_else(|error| panic!("{repo}: {error}"));
        assert_eq!(
            pinned.text(&result),
            golden("graph", repo, "refs.txt"),
            "{repo}"
        );
    }
}

// The refs text after its receipt line.
fn refs_body(repo: &str, target: &str) -> String {
    let pinned = Pinned::new(repo);
    let request = RefsRequest {
        target: target.to_owned(),
        limit: int("50"),
        shared_callers: false,
    };
    let noted = app::refs(&pinned.services, &pinned.reading(), &request);
    let result = noted
        .value
        .unwrap_or_else(|error| panic!("{repo}: {error}"));
    let text = pinned.text(&result);
    text.split_once('\n')
        .map_or(text.clone(), |(_, body)| body.to_owned())
}

#[test]
fn a_qualified_name_names_its_one_definition_in_the_header() {
    let expected = "3 references to py:pricing.py::PriceBook.quote\n\
                    no row has binding evidence: 3 are candidates (unique or \
                    name-only-ambiguous).\n\n\
                    pricing.py  (1 reference, name-only-ambiguous)\n  \
                    [py:pricing.py::PriceBook.price_list] @62\n\n\
                    reports.py  (2 references, name-only-ambiguous)\n  \
                    [py:reports.py::stock_value] @21\n  \
                    [py:reports.py::reorder_report] @39\n";
    assert_eq!(refs_body("repo_py", "PriceBook.quote"), expected);
}

#[test]
fn a_data_key_id_says_in_one_sentence_that_a_key_has_no_fan_in() {
    let expected = "json:config.json::service.port is a data key, and a data key has no fan-in: \
                    Grep finds the spellings of port.\n";
    assert_eq!(
        refs_body("tier2", "json:config.json::service.port"),
        expected
    );
}

#[test]
fn a_name_of_several_definitions_counts_them_and_lists_no_declaration() {
    let expected = "2 references to Invoice, a name that 7 definitions share\n\n\
                    pricing.lua  (2 references, same-file, locals not checked)\n  \
                    [lua:pricing.lua::Invoice.price] @12\n  \
                    (module level) @16\n";
    assert_eq!(refs_body("tier2", "Invoice"), expected);
}

#[test]
fn a_data_file_id_that_names_no_key_gives_the_reason_that_expand_gives() {
    let expected = "config.json keeps an outline of its keys, and service.nope is not in it: the \
                    outline stops at depth 2, at list items and at 500 keys. Read the lines with \
                    read slice, or Grep for the key.\n";
    assert_eq!(
        refs_body("tier2", "json:config.json::service.nope"),
        expected
    );
}
