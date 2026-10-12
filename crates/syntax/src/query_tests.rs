use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::table_tests::fixture;
use crate::{Language, Parsed, Query, QueryRunError, parse};

// The query texts of scripts/syntax_fixtures.py, in its order.
const QUERIES: [(&str, &[&str]); 3] = [
    (
        "python",
        &[
            "(block (string) @statement_string) (module (string) @statement_string)",
            "(call function: (identifier) @_f (#any-of? @_f \"len\" \"print\" \"range\" \"sum\")) @hit",
            "(return_statement (identifier) @_x (#eq? @_x \"total\")) @hit (identifier) @name",
        ],
    ),
    (
        "javascript",
        &[
            "(call_expression function: (member_expression property: (property_identifier) @p)) @hit",
        ],
    ),
    (
        "rust",
        &["(function_item name: (identifier) @name) @function"],
    ),
];

#[derive(Deserialize)]
struct Source {
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryCase {
    name: String,
    language: String,
    runs: Vec<Run>,
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Run {
    matches: Vec<(u32, Vec<(u32, u32)>)>,
    captures: BTreeMap<String, Vec<u32>>,
}

fn run(parsed: &Parsed, query: &Query) -> Run {
    let matches = parsed
        .matches(query)
        .expect("the query runs")
        .into_iter()
        .map(|found| {
            let mut captures: Vec<(u32, u32)> = found
                .captures
                .iter()
                .map(|capture| (capture.index, capture.node.0))
                .collect();
            captures.sort_unstable();
            (found.pattern, captures)
        })
        .collect();
    let found = parsed.captures(query).expect("the query runs");
    let starts: Vec<u32> = found
        .iter()
        .map(|capture| parsed.start_byte(capture.node))
        .collect();
    assert!(
        starts.is_sorted(),
        "captures come in the order of their start bytes"
    );
    let mut captures: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for capture in found {
        let name = query.capture_names()[capture.index as usize].to_owned();
        captures.entry(name).or_default().push(capture.node.0);
    }
    Run { matches, captures }
}

// py-tree-sitter's captures() lists the nodes of a name in no documented order, so the sets compare.
fn sorted(mut run: Run) -> Run {
    for nodes in run.captures.values_mut() {
        nodes.sort_unstable();
    }
    run
}

#[test]
fn every_query_run_equals_the_python_run() {
    let sources: BTreeMap<String, String> = fixture::<Vec<Source>>("parse.json")
        .into_iter()
        .map(|source| (source.name, source.source))
        .collect();
    let cases: Vec<QueryCase> = fixture("queries.json");
    assert!(cases.len() > 90, "the fixture holds {} cases", cases.len());
    for case in cases {
        let language = Language::from_name(&case.language).expect("a known language");
        let (_, texts) = QUERIES
            .iter()
            .find(|(name, _)| *name == case.language)
            .expect("the language has queries");
        let parsed = parse(language, Arc::from(sources[&case.name].as_str())).expect("it parses");
        assert_eq!(case.runs.len(), texts.len(), "{}", case.name);
        for (text, expected) in texts.iter().zip(case.runs) {
            let query = Query::new(language, text).expect("the query compiles");
            let found = sorted(run(&parsed, &query));
            assert_eq!(found, sorted(expected), "{}: {text}", case.name);
        }
    }
}

#[test]
fn a_query_reports_its_names_and_patterns() {
    let query = Query::new(Language::Python, QUERIES[0].1[2]).expect("the query compiles");
    assert_eq!(query.capture_names(), ["_x", "hit", "name"]);
    assert_eq!(query.pattern_count(), 2);
    assert_eq!(query.language(), Language::Python);
}

#[test]
fn a_query_that_does_not_compile_names_its_place() {
    let error = Query::new(Language::Python, "(call)\n(no_such_kind)")
        .err()
        .expect("the query fails");
    assert_eq!((error.row, error.column), (1, 1));
    assert!(error.message.contains("no_such_kind"), "{}", error.message);
}

#[test]
fn a_query_for_another_language_is_refused() {
    let parsed = parse(Language::Rust, Arc::from("fn f() {}\n")).expect("it parses");
    let query = Query::new(Language::Python, "(identifier) @name").expect("the query compiles");
    let refused = QueryRunError::OtherLanguage {
        query: Language::Python,
        tree: Language::Rust,
    };
    assert_eq!(parsed.matches(&query), Err(refused));
    assert_eq!(parsed.captures(&query), Err(refused));
}

#[test]
fn a_run_past_the_match_limit_is_an_error() {
    let source = "x = [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p]\n";
    let parsed = parse(Language::Python, Arc::from(source)).expect("it parses");
    let query = Query::new(Language::Python, "(list (identifier) @a (identifier) @b)")
        .expect("the query compiles");
    assert!(parsed.matches(&query).is_ok());
    assert_eq!(
        parsed.matches_within(&query, 1),
        Err(QueryRunError::MatchLimit)
    );
}
