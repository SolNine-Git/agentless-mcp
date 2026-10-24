//! Each recorded and edge case gives the reference result, compared as the JSON text.

use std::collections::BTreeMap;

use json::{JsonForm, to_text};
use patch::{
    Edit, Elided, Located, Regression, VoteCandidate, apply_edits, equivalence_key, file_key,
    locate, normalized_stream, orientation, parse_blocks, parse_unified_diff, rank,
    resolve_elisions, syntax_delta,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use syntax::Language;

fn cases<T: DeserializeOwned>(file: &str, section: &str) -> Vec<T> {
    text::testing::cases(env!("CARGO_MANIFEST_DIR"), file, section)
}

fn compact<T: serde::Serialize + ?Sized>(value: &T) -> String {
    to_text(value, JsonForm::Compact).expect("a patch value serializes")
}

fn language(name: Option<&str>) -> Option<Language> {
    name.map(|name| Language::from_name(name).unwrap_or_else(|| panic!("no language {name}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRow {
    index: i64,
    path: String,
    search: String,
    replace: String,
}

fn edits(rows: Vec<EditRow>) -> Vec<Edit> {
    rows.into_iter()
        .map(|row| Edit {
            index: row.index,
            path: row.path,
            search: row.search,
            replace: row.replace,
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextCase {
    name: String,
    text: String,
    expect: String,
}

#[test]
fn parse_blocks_gives_the_reference_result() {
    let all: Vec<TextCase> = cases("blocks", "parse_blocks");
    for case in &all {
        assert_eq!(
            compact(&parse_blocks(&case.text)),
            case.expect,
            "{}",
            case.name
        );
    }
    assert!(all.len() > 100, "{} cases", all.len());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffCase {
    name: String,
    text: String,
    expect: DiffExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffExpect {
    result: String,
    notes: Vec<(String, String)>,
}

#[test]
fn parse_unified_diff_gives_the_reference_result() {
    let all: Vec<DiffCase> = cases("unidiff", "parse_unified_diff");
    for case in &all {
        let parsed = parse_unified_diff(&case.text);
        assert_eq!(compact(&parsed.result), case.expect.result, "{}", case.name);
        let notes: Vec<(String, String)> = parsed
            .notes
            .into_iter()
            .map(|note| (note.path, note.reason))
            .collect();
        assert_eq!(notes, case.expect.notes, "{}", case.name);
    }
    assert!(all.len() > 100, "{} cases", all.len());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrientationCase {
    name: String,
    edits: Vec<EditRow>,
    texts: BTreeMap<String, String>,
    expect: String,
}

#[test]
fn orientation_gives_the_reference_refusals() {
    let all: Vec<OrientationCase> = cases("unidiff", "orientation");
    for case in all {
        let found = orientation(&edits(case.edits), &case.texts);
        assert_eq!(compact(&found), case.expect, "{}", case.name);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyCase {
    name: String,
    edits: Vec<EditRow>,
    contents: BTreeMap<String, String>,
    expect: ApplyExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyExpect {
    result: String,
    new_contents: Vec<(String, String)>,
}

#[test]
fn apply_edits_gives_the_reference_outcomes_and_texts() {
    let all: Vec<ApplyCase> = cases("apply", "apply_edits");
    for case in all {
        let applied = apply_edits(&edits(case.edits), &case.contents);
        assert_eq!(compact(&applied), case.expect.result, "{}", case.name);
        assert_eq!(
            applied.new_contents, case.expect.new_contents,
            "{}",
            case.name
        );
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ElisionCase {
    name: String,
    search: String,
    replace: String,
    text: String,
    expect: (String, String, String),
}

#[test]
fn resolve_elisions_gives_the_reference_sides_or_refusal() {
    let all: Vec<ElisionCase> = cases("apply", "resolve_elisions");
    for case in all {
        let (search, replace, reason) = case.expect;
        // A refusal's two sides reach no caller, so only its reason is compared.
        let expected = if reason.is_empty() {
            Ok(Elided { search, replace })
        } else {
            Err(reason)
        };
        let found = resolve_elisions(&case.search, &case.replace, &case.text);
        assert_eq!(found.map_err(str::to_owned), expected, "{}", case.name);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocateCase {
    name: String,
    text: String,
    block: String,
    expect: (usize, usize),
}

#[test]
fn locate_gives_the_reference_line_span() {
    let all: Vec<LocateCase> = cases("apply", "locate");
    for case in all {
        // The reference spells an absent and an ambiguous block alike, as (0, 0).
        let found = match locate(&case.text, &case.block) {
            Located::Unique {
                first_line,
                last_line,
            } => (first_line, last_line),
            Located::Absent | Located::Ambiguous { .. } => (0, 0),
        };
        assert_eq!(found, case.expect, "{}", case.name);
    }
}

#[test]
fn locate_counts_each_overlapping_match() {
    assert_eq!(locate("a\na\na\n", "a"), Located::Ambiguous { matches: 3 });
    assert_eq!(
        locate("a\na\na\n", "a\na"),
        Located::Ambiguous { matches: 2 }
    );
    assert_eq!(locate("a\n", ""), Located::Absent);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamCase {
    name: String,
    text: String,
    language: Option<String>,
    expect: String,
}

#[test]
fn normalized_stream_gives_the_reference_stream() {
    let all: Vec<StreamCase> = cases("keys", "normalized_stream");
    for case in all {
        let found = normalized_stream(&case.text, language(case.language.as_deref()));
        assert_eq!(found.as_deref(), Ok(case.expect.as_str()), "{}", case.name);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileKeyCase {
    name: String,
    old: String,
    new: String,
    language: Option<String>,
    expect: String,
}

#[test]
fn file_key_gives_the_reference_key() {
    let all: Vec<FileKeyCase> = cases("keys", "file_key");
    for case in &all {
        let found = file_key(&case.old, &case.new, language(case.language.as_deref()));
        assert_eq!(found.as_deref(), Ok(case.expect.as_str()), "{}", case.name);
    }
    assert!(all.len() > 100, "{} cases", all.len());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EquivalenceCase {
    name: String,
    changes: Vec<(String, String, String, Option<String>)>,
    expect: String,
}

#[test]
fn equivalence_key_over_the_file_keys_gives_the_reference_key() {
    let all: Vec<EquivalenceCase> = cases("keys", "equivalence_key");
    for case in all {
        let keys: BTreeMap<String, String> = case
            .changes
            .iter()
            .map(|(path, old, new, name)| {
                let key = file_key(old, new, language(name.as_deref())).expect("both texts parse");
                (path.clone(), key)
            })
            .collect();
        assert_eq!(equivalence_key(&keys), case.expect, "{}", case.name);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeltaCase {
    name: String,
    old: String,
    new: String,
    language: Option<String>,
    expect: String,
}

#[test]
fn syntax_delta_gives_the_reference_verdict() {
    let all: Vec<DeltaCase> = cases("keys", "syntax_delta");
    for case in &all {
        let verdict = syntax_delta(&case.old, &case.new, language(case.language.as_deref()))
            .expect("both texts parse");
        assert_eq!(compact(&verdict), case.expect, "{}", case.name);
    }
    assert!(all.len() > 100, "{} cases", all.len());
}

// A missing field still fails: serde cannot deny unknown fields beside a flattened struct.
#[derive(Deserialize)]
struct CandidateRow {
    id: String,
    index: i64,
    equivalence_key: Option<String>,
    #[serde(flatten)]
    run: RunFlags,
    #[serde(flatten)]
    passes: PassFlags,
}

#[derive(Deserialize)]
struct RunFlags {
    applied: bool,
    measured: bool,
}

#[derive(Deserialize)]
struct PassFlags {
    regression_passed: bool,
    reproduction_passed: bool,
}

// The reference's two regression flags as one value. Not measured wins: the ladder excludes such
// a candidate before it reads whether the regression passed.
fn candidate(row: CandidateRow) -> VoteCandidate {
    let regression = match (row.run.measured, row.passes.regression_passed) {
        (false, _) => Regression::NotMeasured,
        (true, false) => Regression::Failed,
        (true, true) => Regression::Passed,
    };
    VoteCandidate {
        id: row.id,
        index: row.index,
        applied: row.run.applied,
        equivalence_key: row.equivalence_key,
        regression,
        reproduction_passed: row.passes.reproduction_passed,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RankCase {
    name: String,
    candidates: Vec<CandidateRow>,
    repro_valid: bool,
    expect: String,
}

#[test]
fn rank_gives_the_reference_report() {
    let all: Vec<RankCase> = cases("vote", "rank");
    for case in all {
        let candidates: Vec<VoteCandidate> = case.candidates.into_iter().map(candidate).collect();
        let report = rank(&candidates, case.repro_valid);
        assert_eq!(compact(&report), case.expect, "{}", case.name);
    }
}
