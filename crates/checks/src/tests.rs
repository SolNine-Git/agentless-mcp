use std::collections::{BTreeMap, BTreeSet};

use metrics::{Block, Unmeasurable};
use serde_json::{Map, Value, json};
use syntax::{Language, Unparsable};
use text::testing::document;

use crate::finding::{Finding, PairFinding, Severity, sorted};
use crate::globs::in_boundary;
use crate::pair::{COMPLEXITY_REASONS, side_reason};
use crate::{
    CloneIndex, Error, IndexedFiles, Observed, Outcome, Pair, SideError, SideText, TextPair,
    UnitState, clone_delta, clone_index, complexity_delta, contract_drift, measure_blocks,
    pair_language,
};

pub(crate) struct Fixture {
    texts: BTreeMap<String, String>,
    document: Value,
}

impl Fixture {
    pub(crate) fn load(file: &str) -> Fixture {
        let document: Value = document(env!("CARGO_MANIFEST_DIR"), file);
        let texts = serde_json::from_value(document["texts"].clone()).expect("a text table");
        Fixture { texts, document }
    }

    pub(crate) fn text(&self, key: &Value) -> Option<String> {
        key.as_str().map(|key| self.texts[key].clone())
    }

    pub(crate) fn cases(&self) -> &[Value] {
        self.document["cases"].as_array().expect("a case list")
    }

    fn pairs(&self, case: &Value) -> Vec<Pair> {
        let rows = case["pairs"].as_array().expect("a pair list");
        let texts = rows
            .iter()
            .map(|row| TextPair {
                path: row["path"].as_str().expect("a path").to_owned(),
                old: self.text(&row["old"]),
                new: self.text(&row["new"]),
            })
            .collect();
        Pair::measure_all(texts, |sides| sides.iter().map(SideText::measure).collect())
    }

    fn index(&self, spec: &Value) -> Result<CloneIndex, Error> {
        if spec["no_tag_cache"] == json!(true) {
            return Ok(CloneIndex::NoTagCache);
        }
        if let Some(reason) = spec["unlisted"].as_str() {
            let unlisted = Some(reason.to_owned());
            return Ok(CloneIndex::Built(IndexedFiles {
                unlisted,
                ..IndexedFiles::default()
            }));
        }
        self.built(spec).map(CloneIndex::Built)
    }

    fn built(&self, spec: &Value) -> Result<IndexedFiles, Error> {
        let table = spec["texts"].as_object().expect("an index text table");
        let texts: BTreeMap<String, String> = table
            .iter()
            .map(|(path, key)| (path.clone(), self.text(key).expect("an index text")))
            .collect();
        let unread: BTreeMap<String, String> =
            serde_json::from_value(spec["unread"].clone()).expect("an unread table");
        let budget: Option<Vec<bool>> =
            serde_json::from_value(spec["budget"].clone()).expect("a budget list or null");
        let mut answers = budget.map(Vec::into_iter);
        let in_budget = move || {
            answers.as_mut().is_none_or(|answers| {
                answers
                    .next()
                    .expect("the index asked its budget more often than the recorded run did")
            })
        };
        let blocks_of = |_: &str, text: &str, language: Language| measure_blocks(language, text);
        clone_index(&texts, &unread, blocks_of, in_budget)
    }

    fn run(&self, case: &Value) -> Value {
        let pairs = self.pairs_of(case);
        let refs: Vec<&Pair> = pairs.iter().collect();
        match case["check"].as_str().expect("a check name") {
            "complexity_delta" => {
                let threshold = case["threshold"].as_u64().expect("a threshold");
                result(complexity_delta(
                    &refs,
                    u32::try_from(threshold).expect("a u32"),
                ))
            }
            "contract_drift" => {
                let globs: Vec<String> =
                    serde_json::from_value(case["globs"].clone()).expect("a glob list");
                result(contract_drift(&refs, &globs))
            }
            "clone_delta" => result(
                self.index(&case["index"])
                    .and_then(|index| clone_delta(&refs, &index, &BTreeSet::new())),
            ),
            "clone_index" => match self.built(&case["index"]) {
                Ok(built) => json!({ "outcome": index_json(&built) }),
                Err(error) => raised(&error),
            },
            other => panic!("unknown check {other}"),
        }
    }

    fn pairs_of(&self, case: &Value) -> Vec<Pair> {
        if case["pairs"].is_null() {
            return Vec::new();
        }
        self.pairs(case)
    }
}

fn result(outcome: Result<Outcome, Error>) -> Value {
    match outcome {
        Ok(outcome) => json!({ "outcome": serde_json::to_value(outcome).expect("JSON") }),
        Err(error) => raised(&error),
    }
}

fn raised(error: &Error) -> Value {
    match error {
        Error::RepeatedPath { .. } | Error::NotIndexable { .. } => {
            json!({ "raised": "ValueError" })
        }
        Error::Side { .. } => json!({ "raised": error.to_string() }),
    }
}

fn index_json(built: &IndexedFiles) -> Value {
    let files: Map<String, Value> = built
        .files
        .iter()
        .map(|(path, blocks)| (path.clone(), blocks.iter().map(block_json).collect()))
        .collect();
    json!({ "files": files, "unindexed": built.unindexed })
}

fn block_json(block: &Block) -> Value {
    json!({
        "start_line": block.start_line,
        "end_line": block.end_line,
        "key": block.key.to_string(),
        "exact": block.exact.to_string(),
        "function": block.function,
        "tokens": block.tokens,
    })
}

fn assert_reference_outcomes(name: &str, at_least: usize) {
    let fixture = Fixture::load(name);
    let total = fixture.cases().len();
    assert!(total >= at_least, "only {total} cases in {name}");
    let mut differ = Vec::new();
    for case in fixture.cases() {
        let actual = fixture.run(case);
        if actual != case["expect"] {
            differ.push(format!(
                "{} {}\n  reference: {}\n  rust:      {}",
                case["check"], case["name"], case["expect"], actual
            ));
        }
    }
    assert!(
        differ.is_empty(),
        "{} of {total} differ:\n{}",
        differ.len(),
        differ.join("\n")
    );
}

#[test]
fn the_recorded_test_cases_give_the_reference_outcomes() {
    assert_reference_outcomes("recorded", 100);
}

#[test]
fn the_seeded_edits_of_every_tier1_language_give_the_reference_outcomes() {
    assert_reference_outcomes("seeded", 500);
}

#[test]
fn each_path_gets_the_reference_language() {
    let fixture = Fixture::load("recorded");
    let rows = fixture.document["pair_language"]
        .as_array()
        .expect("a language table");
    assert!(!rows.is_empty());
    for row in rows {
        let path = row[0].as_str().expect("a path");
        let language = pair_language(path).map(Language::name);
        assert_eq!(language, row[1].as_str(), "{path}");
    }
}

#[test]
fn each_boundary_glob_row_matches_as_the_reference_matches_it() {
    let fixture = Fixture::load("recorded");
    let rows = fixture.document["boundary"]
        .as_array()
        .expect("a boundary table");
    assert!(rows.len() >= 20);
    for row in rows {
        let path = row[0].as_str().expect("a path");
        let globs: Vec<String> = serde_json::from_value(row[1].clone()).expect("a glob list");
        assert_eq!(
            Some(in_boundary(path, &globs)),
            row[2].as_bool(),
            "{path} {globs:?}"
        );
    }
}

#[test]
fn the_glob_grid_matches_as_the_reference_matches_it() {
    let fixture = Fixture::load("recorded");
    let grid = &fixture.document["boundary_grid"];
    let globs: Vec<String> = serde_json::from_value(grid["globs"].clone()).expect("globs");
    let paths: Vec<String> = serde_json::from_value(grid["paths"].clone()).expect("paths");
    let rows: Vec<String> = serde_json::from_value(grid["expected"].clone()).expect("rows");
    assert!(globs.len() * paths.len() > 100_000);
    let mut differ = Vec::new();
    for (glob, row) in globs.iter().zip(&rows) {
        for (path, bit) in paths.iter().zip(row.chars()) {
            if in_boundary(path, std::slice::from_ref(glob)) != (bit == '1') {
                differ.push(format!("{glob:?} {path:?}"));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

fn row(path: &str) -> PairFinding {
    PairFinding {
        finding: Finding {
            check: crate::COMPLEXITY_DELTA,
            severity: Severity::Warning,
            message: String::new(),
            path: path.to_owned(),
            line: 1,
            evidence: String::new(),
        },
        detail: None,
    }
}

// UTF-16 puts U+10000 before U+FFFF; code points and UTF-8 bytes put it after.
#[test]
fn findings_order_paths_by_code_point_past_the_basic_plane() {
    let order = sorted(vec![row("\u{10000}.py"), row("\u{ffff}.py")]);
    let paths: Vec<&str> = order
        .iter()
        .map(|found| found.finding.path.as_str())
        .collect();
    assert_eq!(paths, ["\u{ffff}.py", "\u{10000}.py"]);
}

fn state(unit: &str) -> UnitState {
    UnitState {
        path: "a.py".to_owned(),
        unit: unit.to_owned(),
        line: 1,
        cc: 11,
        sloc: 2,
    }
}

#[test]
fn merging_keeps_each_field_this_state_holds_and_fills_the_rest() {
    let mine = Observed {
        units_over_threshold: Some(vec![state("mine")]),
        ..Observed::default()
    };
    let other = Observed {
        units_over_threshold: Some(vec![state("other")]),
        contract_keys: Some(Vec::new()),
        ..Observed::default()
    };
    let merged = mine.merged(other);
    assert_eq!(merged.units_over_threshold, Some(vec![state("mine")]));
    assert_eq!(merged.contract_keys, Some(Vec::new()));
    assert_eq!(merged.units_changed, None);
}

#[test]
fn every_severity_is_listed_once_in_the_order_the_refusals_name_them() {
    let names: Vec<&str> = Severity::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(names, ["advisory", "warning", "not_checked"]);
}

#[test]
fn a_side_without_a_usable_tree_stops_the_run_with_its_path() {
    let language = Language::Python;
    let error = SideError::Unparsable(Unparsable::NoTree { language });
    let stopped = side_reason("src/a.py", &error, &COMPLEXITY_REASONS)
        .expect_err("a side with no tree is not a row");
    assert!(matches!(&stopped, Error::Side { path, .. } if path == "src/a.py"));
    assert_eq!(
        stopped.to_string(),
        "src/a.py: the Python parser returned no tree"
    );
}

// Recorded deviation: the reference crashes on a side whose only error is a hidden MISSING token.
#[test]
fn a_side_with_only_a_hidden_error_does_not_parse_where_the_reference_crashes() {
    let source = "int f(void) { return 0; }\n#define X 1";
    let refused = measure_blocks(Language::C, source).err();
    let line = Unmeasurable::DoesNotParse { line: 2 };
    assert_eq!(refused, Some(SideError::Unmeasurable(line)));
    let pair = TextPair {
        path: "src/a.c".to_owned(),
        old: None,
        new: Some(source.to_owned()),
    };
    let pairs = Pair::measure_all(vec![pair], |texts| {
        texts.iter().map(SideText::measure).collect()
    });
    let outcome = complexity_delta(&[&pairs[0]], 10).expect("a row, not an error");
    let messages: Vec<&str> = outcome
        .findings
        .iter()
        .map(|row| row.finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "not checked: complexity_delta did not compare src/a.c, because its text after the \
             change cannot be measured. src/a.c does not parse: the first syntax error is at line \
             2. Complexity is measured only for a file that parses. Fix the syntax error, then \
             measure the file again."
        ]
    );
}

#[test]
#[should_panic(expected = "the map returns one side per text")]
fn a_map_that_returns_fewer_sides_than_texts_panics() {
    let pair = TextPair {
        path: "a.py".to_owned(),
        old: Some("x = 1\n".to_owned()),
        new: Some("x = 2\n".to_owned()),
    };
    let fewer = |texts: &[SideText<'_>]| texts.iter().skip(1).map(SideText::measure).collect();
    let _pairs = Pair::measure_all(vec![pair], fewer);
}

// A five-line function of 71 leaf tokens with a two-line loop inside it.
fn total() -> String {
    "def total(items, scale=1, offset=None):\n    out = 0 * scale + len(items) - (offset or 0)\n    \
     for item in sorted(items, key=abs, reverse=True):\n        out += item * scale - 0 + \
     max(item, 0)\n    return round(out, 2) if out else 0\n"
        .to_owned()
}

const LOOP: &str = "for row in sorted(rows, key=len, reverse=True)[:limit]:\n    print(row, sep=', ', \
                    end='', flush=True)\n    found += len(row) * limit - min(len(row), limit)\n";

// A function whose three-line loop of 54 leaf tokens is a candidate of its own.
fn looped(name: &str) -> String {
    let body: String = LOOP.lines().flat_map(|line| ["    ", line, "\n"]).collect();
    format!("def {name}(rows, limit=10):\n    found = 0\n{body}    return found\n")
}

// The clone_delta findings of `new` as a new b.py against an index of a.py holding `old`.
fn overlap_rows(new: &str, old: &str, reported: &[(&str, u32)]) -> Vec<(String, u32)> {
    let texts = BTreeMap::from([("a.py".to_owned(), old.to_owned())]);
    let blocks_of = |_: &str, text: &str, language: Language| measure_blocks(language, text);
    let index = clone_index(&texts, &BTreeMap::new(), blocks_of, || true).expect("an index");
    let pair = TextPair {
        path: "b.py".to_owned(),
        old: None,
        new: Some(new.to_owned()),
    };
    let pairs = Pair::measure_all(vec![pair], |texts| {
        texts.iter().map(SideText::measure).collect()
    });
    let reported: BTreeSet<(String, u32)> = reported
        .iter()
        .map(|(path, line)| ((*path).to_owned(), *line))
        .collect();
    let outcome = clone_delta(&[&pairs[0]], &CloneIndex::Built(index), &reported);
    let findings = outcome.expect("an outcome").findings;
    findings
        .into_iter()
        .map(|row| (row.finding.path, row.finding.line))
        .collect()
}

#[test]
fn a_function_near_duplicates_reported_gives_no_clone_row() {
    assert_eq!(overlap_rows(&total(), &total(), &[("b.py", 1)]), []);
}

#[test]
fn a_block_nested_in_a_reported_function_gives_no_clone_row() {
    let rows = overlap_rows(&looped("again"), &looped("scan"), &[("b.py", 1)]);
    assert_eq!(rows, []);
}

#[test]
fn a_loop_at_a_reported_line_is_still_a_clone_row() {
    let rows = overlap_rows(LOOP, LOOP, &[("b.py", 1)]);
    assert_eq!(rows, [("b.py".to_owned(), 1)]);
}

#[test]
fn a_function_at_another_reported_line_is_still_a_clone_row() {
    let rows = overlap_rows(&total(), &total(), &[("b.py", 7)]);
    assert_eq!(rows, [("b.py".to_owned(), 1)]);
}
