use syntax::Language;

use super::{WarmupRequest, WarmupRow, warmup};

fn request(names: &[&str]) -> WarmupRequest {
    WarmupRequest {
        languages: names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

#[test]
fn a_warmup_without_names_probes_every_linked_grammar_cleanly() {
    let report = warmup(&request(&[]));
    assert!(!report.requested);
    assert_eq!(report.rows.len(), Language::ALL.len());
    assert!(report.rows.iter().all(WarmupRow::usable));
    assert_eq!((report.fatal(), report.degraded().count()), (0, 0));
}

// Ledger deviation: nothing downloads, so an unknown name is a row that says no grammar has it.
#[test]
fn a_named_unknown_language_is_one_fatal_tier_two_row() {
    let report = warmup(&request(&["python", "nope", "python"]));
    assert!(report.requested);
    assert_eq!(report.rows.len(), 3);
    assert_eq!(report.rows[1], WarmupRow::Unknown("nope".to_owned()));
    assert_eq!(report.rows[1].tier(), 2);
    assert_eq!((report.fatal(), report.degraded().count()), (1, 1));
}
