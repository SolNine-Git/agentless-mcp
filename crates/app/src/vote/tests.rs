use json::{JsonForm, to_text};
use serde_json::Value;

use super::vote;
use crate::testing::MANIFEST_DIR;

#[test]
fn each_document_that_reads_ranks_as_the_reference_ranks_it() {
    let cases: Vec<(String, Value)> =
        io::testing::cases(MANIFEST_DIR, "validate/verdicts", "texts");
    let ranked: Vec<(&String, &str)> = cases
        .iter()
        .filter_map(|(text, expected)| Some((text, expected.get("vote")?.as_str()?)))
        .collect();
    assert!(ranked.len() >= 8, "{} ranked documents", ranked.len());
    for (text, expected) in ranked {
        let found = vote(text).unwrap_or_else(|error| panic!("{text:?}: {error}"));
        let shown = to_text(&found.report, JsonForm::Compact).expect("a report serializes");
        assert_eq!(shown, expected, "{text:?}");
    }
}

#[test]
fn a_document_the_reader_refuses_has_no_vote() {
    let refused = vote("").expect_err("an empty document is refused");

    assert_eq!(
        refused.to_string(),
        "the verdicts file is empty: run `agentless-mcp validate` to produce one"
    );
}
