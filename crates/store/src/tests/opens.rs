use std::path::Path;
use std::time::Duration;

use super::cases::{BUSY, OpenCase, ROOT, Scratch, cases, exclusive_writer, usable};
use crate::open::open;
use crate::{
    CacheState, CacheStatus, Discarded, Refusal, Source, SqlClass, SqlError, discard, open_source,
};

#[test]
fn each_open_gives_the_reference_receipt_and_leaves_the_reference_files() {
    for case in cases().opens {
        let scratch = Scratch::new();
        scratch.damage(&case.damage, "");
        let (kind, state) = opened(&case, &scratch.database);
        let status = CacheStatus {
            path: scratch.database.clone(),
            state,
        };
        let found = (kind, status.receipt(), status.note());
        let expected = (case.kind.as_str(), case.receipt, case.note);
        assert_eq!(found, expected, "{}", case.name);
        assert_eq!(scratch.database.exists(), case.exists, "{}", case.name);
    }
}

// The app's sequence: open, discard a refused database that the refusal discards, then report.
fn opened(case: &OpenCase, database: &Path) -> (&'static str, CacheState) {
    let kind = case.damage[0].as_str().expect("a damage kind");
    let holder = (kind == "locked").then(|| exclusive_writer(database));
    let busy = holder.as_ref().map_or(BUSY, |_| Duration::ZERO);
    let source = if kind == "bypass" {
        open_source(database, ROOT, true)
    } else {
        open(database, ROOT, busy)
    };
    drop(holder);
    match source {
        Source::Bypassed => ("bypassed", CacheState::Bypassed),
        Source::Absent => ("absent", CacheState::Absent { note: None }),
        Source::Refused(refusal) => {
            if refusal.discards() {
                let discarded = discard(database);
                assert!(matches!(discarded, Discarded::Removed), "{}", case.name);
            }
            let note = Some(refusal);
            ("refused", CacheState::Absent { note })
        }
        Source::Usable(found) => {
            let state = CacheState::Indexed {
                generation: found.generation().to_owned(),
                repo_generation: case.repo_generation.clone(),
                repo_root: ROOT.to_owned(),
            };
            ("usable", state)
        }
    }
}

#[test]
fn the_fixture_opens_at_its_generation_with_its_row_counts() {
    let cases = cases();
    let (generation, counts) = &cases.fixture[0];
    let scratch = Scratch::new();
    let found = usable(open(&scratch.database, ROOT, BUSY), "fixture");
    assert_eq!(found.generation(), generation);
    let rows = found.row_counts().expect("the fixture counts its rows");
    assert_eq!([rows.files, rows.tags, rows.imports, rows.refs], *counts);
}

#[test]
fn an_unreadable_file_names_the_failed_open_without_its_path() {
    let scratch = Scratch::new();
    // Where modes do not bind, as for root, the open reads the file.
    let binds = scratch.tree.modes_bind();
    scratch
        .tree
        .set_mode("tags.db", 0o000)
        .expect("the mode is set");
    let source = open(&scratch.database, ROOT, BUSY);
    scratch
        .tree
        .set_mode("tags.db", 0o600)
        .expect("the mode is set back");
    if !binds {
        assert!(matches!(source, Source::Usable(_)), "{source:?}");
        return;
    }
    let Source::Refused(refusal) = source else {
        panic!("an unreadable file is refused: {source:?}");
    };
    let expected = "cache unreadable: unable to open database file";
    assert_eq!(refusal.to_string(), expected);
    assert!(!refusal.discards());
}

#[test]
fn a_stat_failure_other_than_a_missing_file_is_unreadable() {
    let scratch = Scratch::new();
    // Where modes do not bind, as for root, the stat finds no database.
    let binds = scratch.tree.modes_bind();
    let tree = &scratch.tree;
    tree.create_dir("closed").expect("a directory");
    tree.set_mode("closed", 0o000).expect("the mode is set");
    let source = open(&tree.join("closed/tags.db"), ROOT, BUSY);
    tree.set_mode("closed", 0o700)
        .expect("the mode is set back");
    if !binds {
        assert!(matches!(source, Source::Absent), "{source:?}");
        return;
    }
    let Source::Refused(refusal) = source else {
        panic!("a database behind a closed directory is refused: {source:?}");
    };
    assert_eq!(refusal.to_string(), "cache unreadable: Permission denied");
}

#[test]
fn only_a_failure_of_the_file_itself_discards_it() {
    let keeps = [
        SqlClass::Operational,
        SqlClass::Interface,
        SqlClass::Value,
        SqlClass::Type,
        SqlClass::JsonDecode,
    ];
    let discards = [
        SqlClass::Database,
        SqlClass::Integrity,
        SqlClass::Internal,
        SqlClass::Data,
    ];
    for class in keeps {
        let refusal = Refusal::from(SqlError::new(class, "m"));
        assert!(matches!(refusal, Refusal::Unreadable(_)), "{class:?}");
    }
    for class in discards {
        let refusal = Refusal::from(SqlError::new(class, "m"));
        assert!(matches!(refusal, Refusal::Corrupt(_)), "{class:?}");
    }
}

#[test]
fn a_result_code_maps_to_its_class_through_its_primary_code() {
    let codes = [
        (1, SqlClass::Operational),
        (5, SqlClass::Operational),
        (261, SqlClass::Operational),
        (1544, SqlClass::Operational),
        (2, SqlClass::Internal),
        (11, SqlClass::Database),
        (26, SqlClass::Database),
        (18, SqlClass::Data),
        (19, SqlClass::Integrity),
        (2067, SqlClass::Integrity),
        (21, SqlClass::Interface),
        (25, SqlClass::Interface),
    ];
    for (code, class) in codes {
        assert_eq!(SqlClass::of_code(code), class, "code {code}");
    }
}
