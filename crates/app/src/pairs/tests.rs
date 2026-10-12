use checks::{Pair, SideError, SideText, TextPair};
use syntax::Language;

use super::{Stored, built_index, measured_blocks, stored_blocks};
use crate::testing::Sandbox;

const SOURCE: &str =
    "def total(items):\n    out = 0\n    for item in items:\n        out += item\n    return out\n";
const BROKEN: &str = "def total(items:\n    return\n";

fn measured() -> Vec<metrics::Block> {
    checks::measure_blocks(Language::Python, SOURCE).expect("the source measures")
}

fn no_measure() -> Result<Vec<metrics::Block>, SideError> {
    panic!("a stored row needs no measure")
}

fn pair(old: &str, new: &str) -> Pair {
    let text = TextPair {
        path: "src/total.py".to_owned(),
        old: Some(old.to_owned()),
        new: Some(new.to_owned()),
    };
    let mut pairs = Pair::measure_all(vec![text], |texts| {
        texts.iter().map(SideText::measure).collect()
    });
    pairs.pop().expect("one pair")
}

#[test]
fn a_stored_hit_is_used_as_it_is() {
    let mut warnings = Vec::new();

    let found = stored_blocks(Stored::Hit(Vec::new()), &mut warnings, no_measure);

    assert_eq!(found, Ok(Vec::new()));
    assert_eq!(warnings, Vec::<String>::new());
}

#[test]
fn a_stored_reason_is_the_gap_text_verbatim() {
    let mut warnings = Vec::new();
    let stored = Stored::Unmeasured("the source does not parse at line 3".to_owned());

    let found = stored_blocks(stored, &mut warnings, no_measure);

    let reason = "the source does not parse at line 3".to_owned();
    assert_eq!(found, Err(SideError::Recorded(reason)));
}

#[test]
fn a_miss_and_an_unreadable_row_measure_the_text_in_memory() {
    let mut warnings = Vec::new();
    let unreadable = Stored::Unreadable("tag cache rows are unreadable".to_owned());
    let measure = || measured_blocks(None, SOURCE, Language::Python);

    let missed = stored_blocks(Stored::Miss, &mut warnings, measure);
    let read = stored_blocks(unreadable, &mut warnings, measure);

    assert_eq!(missed, Ok(measured()));
    assert_eq!(read, Ok(measured()));
    assert_eq!(warnings, ["tag cache rows are unreadable"]);
}

#[test]
fn a_side_with_the_same_text_gives_what_a_measure_in_memory_gives() {
    let other = "def other():\n    return 1\n";
    for (old, new, text) in [
        (SOURCE, other, SOURCE),
        (other, SOURCE, SOURCE),
        (BROKEN, other, BROKEN),
        (other, other, SOURCE),
    ] {
        let found = measured_blocks(Some(&pair(old, new)), text, Language::Python);
        let expected = checks::measure_blocks(Language::Python, text);
        assert_eq!(found, expected, "{text:?}");
    }
    assert!(checks::measure_blocks(Language::Python, BROKEN).is_err());
}

#[test]
fn a_pair_finds_only_a_side_with_the_same_text() {
    let found = pair(SOURCE, BROKEN);

    assert_eq!(found.measured_blocks(SOURCE), Some(Ok(measured())));
    assert!(matches!(found.measured_blocks(BROKEN), Some(Err(_))));
    assert_eq!(found.measured_blocks("x = 1\n"), None);
}

#[test]
fn the_walk_warnings_reach_the_lint_diagnostics() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", SOURCE)]);
    sandbox.apply(&["latin_name", "repo/bad\u{ff}.py", "z = 1\n"].map(serde_json::Value::from));
    let services = sandbox.services(sandbox.path());
    let mut lookup = |_: &str, _: &str, _| Stored::Miss;

    let built = built_index(&services, &root, &mut lookup, &[]);

    let files = built.value.expect("an index");
    assert_eq!(files.files.keys().collect::<Vec<_>>(), ["a.py"]);
    let warning = format!(
        "git listed 1 path(s) under {} whose names are not valid UTF-8. The tree omits them.",
        root.display()
    );
    assert_eq!(built.diagnostics, [warning]);
}

#[test]
fn a_root_the_walk_refuses_is_one_unlisted_reason() {
    let sandbox = Sandbox::new();
    let services = sandbox.services(sandbox.path());
    let missing = sandbox.path().join("missing");
    let mut lookup = |path: &str, _: &str, _| panic!("{path}: nothing to look up");

    let built = built_index(&services, &missing, &mut lookup, &[]);

    let files = built.value.expect("an index");
    let unlisted = format!("not a directory: {}", missing.display());
    assert_eq!(files.unlisted, Some(unlisted));
    assert!(files.files.is_empty());
    assert_eq!(built.diagnostics, Vec::<String>::new());
}
