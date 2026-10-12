use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use metrics::Block;
use serde_json::{Map, Value, json};
use syntax::Language;

use crate::finding::{Finding, PairFinding, Severity, sorted};
use crate::globs::in_boundary;
use crate::{
    CloneIndex, Error, IndexedFiles, Observed, Outcome, Pair, SideText, TextPair, UnitState,
    clone_delta, clone_index, complexity_delta, contract_drift, measure_blocks, pair_language,
};

struct Fixture {
    texts: BTreeMap<String, String>,
    document: Value,
}

impl Fixture {
    fn load(name: &str) -> Fixture {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name);
        let text =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let document: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let texts = serde_json::from_value(document["texts"].clone()).expect("a text table");
        Fixture { texts, document }
    }

    fn text(&self, key: &Value) -> Option<String> {
        key.as_str().map(|key| self.texts[key].clone())
    }

    fn cases(&self) -> &[Value] {
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
                    .expect("the index asked its budget more often than Python did")
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
                    .and_then(|index| clone_delta(&refs, &index)),
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
        other => json!({ "raised": other.to_string() }),
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

fn assert_python_outcomes(name: &str, at_least: usize) {
    let fixture = Fixture::load(name);
    let total = fixture.cases().len();
    assert!(total >= at_least, "only {total} cases in {name}");
    let mut differ = Vec::new();
    for case in fixture.cases() {
        let actual = fixture.run(case);
        if actual != case["expect"] {
            differ.push(format!(
                "{} {}\n  python: {}\n  rust:   {}",
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
fn the_recorded_test_cases_give_the_python_outcomes() {
    assert_python_outcomes("recorded.json", 100);
}

#[test]
fn the_seeded_edits_of_every_tier1_language_give_the_python_outcomes() {
    assert_python_outcomes("seeded.json", 500);
}

#[test]
fn each_path_gets_the_language_python_gives_it() {
    let fixture = Fixture::load("recorded.json");
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
fn each_boundary_glob_row_matches_as_in_python() {
    let fixture = Fixture::load("recorded.json");
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
fn the_glob_grid_matches_as_in_python() {
    let fixture = Fixture::load("recorded.json");
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
