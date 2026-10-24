use std::path::PathBuf;

use app::{EvictionReport, IndexFailure, IndexReport, IndexSkip};
use serde::Deserialize;

use crate::index_text;

// The store crate's index report cases hold the reference text of each report.
const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../store");
const DATABASE: &str = "/cache/0123456789abcdef/agentless.db";

#[derive(Deserialize)]
struct ReportCase {
    name: String,
    values: Values,
    text: String,
}

#[derive(Deserialize)]
struct Values {
    indexed: usize,
    reused: usize,
    pruned: usize,
    files: u64,
    tags: u64,
    imports: u64,
    refs: u64,
    failures: usize,
    skips: usize,
    evicted: (u64, u64),
}

// The report that the store generator builds from the same values.
fn report_of(values: &Values) -> IndexReport {
    IndexReport {
        database: PathBuf::from(DATABASE),
        generation: "0123abcd".to_owned(),
        indexed: values.indexed,
        reused: values.reused,
        pruned: values.pruned,
        files: values.files,
        tags: values.tags,
        imports: values.imports,
        refs: values.refs,
        failures: (0..values.failures)
            .map(|i| IndexFailure {
                path: format!("f{i}.py"),
                reason: format!("reason {i}"),
            })
            .collect(),
        skipped_files: (0..values.skips)
            .map(|i| IndexSkip {
                path: format!("s{i}.py"),
                reason: format!("skip {i}"),
            })
            .collect(),
        evicted: EvictionReport {
            databases: values.evicted.0,
            size: values.evicted.1,
        },
    }
}

#[test]
fn each_index_report_prints_the_reference_text() {
    let reports: Vec<ReportCase> = text::testing::cases(STORE, "index/report", "reports");
    assert!(reports.iter().any(|case| case.values.failures > 10));
    for case in &reports {
        assert_eq!(
            index_text(&report_of(&case.values)),
            case.text,
            "{}",
            case.name
        );
    }
}

#[test]
fn a_line_break_in_a_file_name_stays_inside_its_row() {
    let values = Values {
        indexed: 0,
        reused: 0,
        pruned: 0,
        files: 0,
        tags: 0,
        imports: 0,
        refs: 0,
        failures: 0,
        skips: 1,
        evicted: (0, 0),
    };
    let mut report = report_of(&values);
    report.skipped_files[0] = IndexSkip {
        path: "a\nindexed 9\rb.py".to_owned(),
        reason: "too large\x1b[0m".to_owned(),
    };
    let text = index_text(&report);
    assert_eq!(text.lines().count(), 2, "{text:?}");
    assert!(text.ends_with("  warning: a\\nindexed 9\\rb.py: too large\\x1b[0m\n"));
}
