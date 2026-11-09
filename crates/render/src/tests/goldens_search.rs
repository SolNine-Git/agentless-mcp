use app::IntArg;
use app::search::{DEFAULT_SEARCH_LIMIT, SearchRequest};
use serde_json::Value;

use super::goldens_trace::{Rig, assert_each_golden_has_a_request, compared, golden, requests};
use crate::{SearchAnswer, search_refusal};

fn search_request(arguments: &Value) -> SearchRequest {
    let text = |name: &str| arguments[name].as_str().map(str::to_owned);
    let count = |name: &str, default: usize| {
        let given = arguments[name].as_u64();
        let count = given.map_or(default, |count| usize::try_from(count).expect("a count"));
        IntArg::from_usize(count)
    };
    SearchRequest {
        pattern: text("pattern"),
        files: text("files"),
        path: text("path"),
        ignore_case: arguments["ignore_case"].as_bool().unwrap_or(false),
        context: count("context", 0),
        limit: count("limit", DEFAULT_SEARCH_LIMIT),
    }
}

// A refusal's golden is its text, which the tool answers with `isError` set.
#[test]
fn the_search_goldens_come_out_byte_for_byte() {
    let mut mismatches = Vec::new();
    for (stem, request) in requests("search") {
        let rig = Rig::new(&request);
        let asked = search_request(&request["arguments"]);
        let noted = app::search::search(&rig.services, &rig.reading(), &asked);
        assert!(
            noted.diagnostics.is_empty(),
            "{stem}: {:?}",
            noted.diagnostics
        );
        match noted.value {
            Ok(searched) => {
                let answer = SearchAnswer {
                    request: &asked,
                    searched: &searched,
                };
                let (text, json) = (rig.text(&answer), rig.json(&answer));
                mismatches.extend(compared("search", &stem, &text, &json));
            }
            Err(error) => {
                let refusal = search_refusal(&error) + "\n";
                if golden("search", &stem, "txt").as_deref() != Some(refusal.as_str()) {
                    mismatches.push(format!("{stem}.txt renders:\n{refusal}"));
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn each_search_golden_has_its_request_beside_it() {
    assert_each_golden_has_a_request("search");
}
