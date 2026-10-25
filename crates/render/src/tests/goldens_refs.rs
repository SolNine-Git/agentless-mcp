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
    let expected = "3 references to py:pricing.py::PriceBook.quote\n\n\
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
