use super::cases::ROOT;
use super::indexed::{Cache, PYTHON, indexed, observed};
use crate::{Refusal, Source, SqlClass, SqlError, open_facts};

const DATABASE: &str = "cache/key/agentless.db";

#[test]
fn an_unreadable_file_names_the_failed_open_without_its_path() {
    let cache = Cache::new();
    indexed(&cache.database, observed(&[("a.py", PYTHON)]), false);
    // Where modes do not bind, as for root, the open reads the file.
    let binds = cache.tree.modes_bind();
    cache
        .tree
        .set_mode(DATABASE, 0o000)
        .expect("the mode is set");
    let source = open_facts(&cache.database, ROOT, false);
    cache
        .tree
        .set_mode(DATABASE, 0o600)
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
    let cache = Cache::new();
    // Where modes do not bind, as for root, the stat finds no database.
    let binds = cache.tree.modes_bind();
    let tree = &cache.tree;
    tree.create_dir("closed").expect("a directory");
    tree.set_mode("closed", 0o000).expect("the mode is set");
    let source = open_facts(&tree.join("closed/agentless.db"), ROOT, false);
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
