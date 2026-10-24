use std::collections::BTreeSet;
use std::iter;

use serde_json::Value;
use syntax::{Language, NodeId, Parsed, Query};

use super::{HIT_CAPTURE, compile, hit_nodes, rules_for};
use crate::pair_language;
use crate::tests::Fixture;

// An unknown predicate compiles and is then ignored, so a typo would widen a rule.
const PREDICATES: [&str; 6] = [
    "eq?",
    "not-eq?",
    "any-of?",
    "not-any-of?",
    "match?",
    "not-match?",
];

// The rule table is static, so this test is the check that every rule can run.
#[test]
fn every_contract_rule_names_a_measured_language_and_compiles_alone() {
    for rule in prompts::contract_rules() {
        let language = Language::from_name(rule.language)
            .filter(|language| language.spec().is_some())
            .unwrap_or_else(|| panic!("rule {} names {}", rule.id, rule.language));
        let query = Query::new(language, rule.query)
            .unwrap_or_else(|error| panic!("rule {} does not compile: {error}", rule.id));
        let names = query.capture_names();
        let stray = names
            .iter()
            .any(|name| *name != HIT_CAPTURE && !name.starts_with('_'));
        assert!(
            names.contains(&HIT_CAPTURE) && !stray,
            "rule {} captures {names:?}",
            rule.id
        );
        let unknown = unknown_predicates(rule.query);
        assert!(unknown.is_empty(), "rule {} uses {unknown:?}", rule.id);
    }
}

#[test]
fn each_language_query_maps_every_pattern_to_its_rule() {
    let mut compiled = 0;
    let measured = Language::ALL
        .into_iter()
        .filter(|language| language.spec().is_some());
    for language in measured {
        let expected: Vec<usize> = rules_for(language)
            .enumerate()
            .flat_map(|(order, rule)| {
                let alone = Query::new(language, rule.query).expect("the rule compiles");
                iter::repeat_n(order, alone.pattern_count())
            })
            .collect();
        let found = compile(language);
        assert_eq!(found.pattern_rules, expected, "{language:?}");
        assert!(found.rules.iter().copied().eq(rules_for(language)));
        assert_eq!(found.hit.is_some(), !found.rules.is_empty(), "{language:?}");
        compiled += found.rules.len();
    }
    assert_eq!(compiled, prompts::contract_rules().len());
}

// The one guard of the combined query: each rule, run alone, finds the hit spans that the combined
// query gives that rule, on every seeded text.
#[test]
fn each_rule_alone_finds_the_hits_that_the_combined_query_gives_it() {
    let mut compared = 0;
    for (language, text) in seeded_sources() {
        let Ok(parsed) = syntax::parse(language, text.as_str().into()) else {
            continue;
        };
        let compiled = compile(language);
        let combined: Vec<Vec<Span>> = hit_nodes(&parsed, &compiled)
            .iter()
            .map(|nodes| nodes.iter().map(|node| span(&parsed, *node)).collect())
            .collect();
        let alone: Vec<Vec<Span>> = compiled
            .rules
            .iter()
            .map(|rule| alone_spans(&parsed, rule.query))
            .collect();
        assert_eq!(combined, alone, "{language:?}:\n{text}");
        compared += usize::from(!compiled.rules.is_empty());
    }
    assert!(compared > 100, "only {compared} texts have rules");
}

type Span = (u32, u32);

fn seeded_sources() -> BTreeSet<(Language, String)> {
    let seeded = Fixture::load("seeded");
    let rows: Vec<&Value> = seeded
        .cases()
        .iter()
        .flat_map(|case| case["pairs"].as_array().into_iter().flatten())
        .collect();
    rows.iter()
        .filter_map(|row| Some((pair_language(row["path"].as_str()?)?, *row)))
        .flat_map(|(language, row)| {
            let texts = [seeded.text(&row["old"]), seeded.text(&row["new"])];
            texts
                .into_iter()
                .flatten()
                .map(move |text| (language, text))
        })
        .collect()
}

fn span(parsed: &Parsed, node: NodeId) -> Span {
    (parsed.start_byte(node), parsed.end_byte(node))
}

fn alone_spans(parsed: &Parsed, query: &str) -> Vec<Span> {
    let query = Query::new(parsed.language(), query).expect("the rule compiles");
    let hit = query
        .capture_names()
        .iter()
        .position(|name| *name == HIT_CAPTURE);
    let spans: BTreeSet<Span> = parsed
        .matches(&query)
        .expect("the rule runs on a tree of its language")
        .iter()
        .flat_map(|found| found.captures.iter())
        .filter(|capture| hit == Some(capture.index as usize))
        .map(|capture| span(parsed, capture.node))
        .collect();
    spans.into_iter().collect()
}

#[test]
fn the_predicate_scan_finds_each_name_after_an_opening_hash() {
    let query = "((x) @a (#eq? @a \"y\") (#nope! @a) (#bad) (# eq?) (#set! k v))";
    assert_eq!(predicate_names(query), ["eq?", "nope!", "set!"]);
    assert_eq!(unknown_predicates(query), ["nope!", "set!"]);
}

// The names that `\(#([A-Za-z-]+[?!])` finds, outside PREDICATES.
fn unknown_predicates(query: &str) -> Vec<&str> {
    predicate_names(query)
        .into_iter()
        .filter(|name| !PREDICATES.contains(name))
        .collect()
}

fn predicate_names(query: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = query;
    while let Some(at) = rest.find("(#") {
        let after = &rest[at + 2..];
        let run = after.len()
            - after
                .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '-')
                .len();
        if run > 0 && matches!(after.as_bytes().get(run), Some(b'?' | b'!')) {
            names.push(&after[..=run]);
            rest = &after[run + 1..];
        } else {
            rest = &rest[at + 1..];
        }
    }
    names
}
