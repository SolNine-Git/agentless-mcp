use std::iter;

use syntax::{Language, Query};

use super::{HIT_CAPTURE, compile, rules_for};

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
