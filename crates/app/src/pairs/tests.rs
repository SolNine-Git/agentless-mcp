use checks::SideError;
use syntax::Language;

use super::{Stored, built_index, stored_blocks};
use crate::testing::Sandbox;

const SOURCE: &str =
    "def total(items):\n    out = 0\n    for item in items:\n        out += item\n    return out\n";

fn measured() -> Vec<metrics::Block> {
    checks::measure_text(Language::Python, SOURCE)
        .expect("the source measures")
        .blocks
}

#[test]
fn a_stored_hit_is_used_as_it_is() {
    let mut warnings = Vec::new();

    let found = stored_blocks(
        Stored::Hit(Vec::new()),
        SOURCE,
        Language::Python,
        &mut warnings,
    );

    assert_eq!(found, Ok(Vec::new()));
    assert_eq!(warnings, Vec::<String>::new());
}

#[test]
fn a_stored_reason_is_the_gap_text_verbatim() {
    let mut warnings = Vec::new();
    let stored = Stored::Unmeasured("the source does not parse at line 3".to_owned());

    let found = stored_blocks(stored, SOURCE, Language::Python, &mut warnings);

    let reason = "the source does not parse at line 3".to_owned();
    assert_eq!(found, Err(SideError::Recorded(reason)));
}

#[test]
fn a_miss_and_an_unreadable_row_measure_the_text_in_memory() {
    let mut warnings = Vec::new();
    let unreadable = Stored::Unreadable("tag cache rows are unreadable".to_owned());

    let missed = stored_blocks(Stored::Miss, SOURCE, Language::Python, &mut warnings);
    let read = stored_blocks(unreadable, SOURCE, Language::Python, &mut warnings);

    assert_eq!(missed, Ok(measured()));
    assert_eq!(read, Ok(measured()));
    assert_eq!(warnings, ["tag cache rows are unreadable"]);
}

#[test]
fn a_root_the_walk_refuses_is_one_unlisted_reason() {
    let sandbox = Sandbox::new();
    let services = sandbox.services(sandbox.path());
    let missing = sandbox.path().join("missing");
    let mut lookup = |path: &str, _: &str, _| panic!("{path}: nothing to look up");

    let (files, warnings) = built_index(&services, &missing, &mut lookup).expect("an index");

    let unlisted = format!("not a directory: {}", missing.display());
    assert_eq!(files.unlisted, Some(unlisted));
    assert!(files.files.is_empty());
    assert_eq!(warnings, Vec::<String>::new());
}
