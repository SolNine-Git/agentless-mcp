// The recorded reference cases of the store index: each case with the result of the reference
// build.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use json::{JsonForm, to_text};
use serde::Deserialize;
use serde_json::Value;
use text::testing::{cases, float};

use crate::warnings::{Failed, unreclaimed};
use crate::{
    CacheEntry, CacheState, CacheStatus, DEFAULT_MAX_CACHE_BYTES, EvictionReport, IndexFailure,
    IndexReport, IndexSkip, Refusal, RowFailure, SqlClass, SqlError, evict_plan, max_cache_bytes,
};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const DATABASE: &str = "/cache/0123456789abcdef/agentless.db";

fn database_of(name: &str) -> PathBuf {
    PathBuf::from(format!("/cache/{name}/agentless.db"))
}

// name, [[entry, size, used at]], limit, keep, now, the victims coldest first
type PlanCase = (
    String,
    Vec<(String, u64, String)>,
    u64,
    String,
    String,
    Vec<String>,
);

#[test]
fn each_eviction_plan_is_the_reference_plan() {
    let plans: Vec<PlanCase> = cases(MANIFEST_DIR, "index/eviction", "plans");
    for (name, entries, limit, keep, now, expected) in plans {
        let entries: Vec<CacheEntry> = entries
            .iter()
            .map(|(entry, size, used_at)| CacheEntry {
                database: database_of(entry),
                size: *size,
                used_at: float(used_at),
            })
            .collect();

        let victims = evict_plan(&entries, limit, &database_of(&keep), float(&now));

        let found: Vec<String> = victims
            .iter()
            .map(|victim| {
                let directory = victim.database.parent().expect("a key directory");
                directory
                    .to_string_lossy()
                    .trim_start_matches("/cache/")
                    .to_owned()
            })
            .collect();
        assert_eq!(found, expected, "{name}");
    }
}

// The bytes of the value or none, the limit as decimal text or none, the warning, and the mark of
// a recorded deviation.
#[derive(Deserialize)]
struct CeilingCase(
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
    Option<String>,
);

// A limit past u64 saturates: like the reference's larger integer, it never evicts.
fn limit_of(decimal: Option<&str>) -> Option<u64> {
    decimal.map(|text| text.parse().unwrap_or(u64::MAX))
}

#[test]
fn each_ceiling_is_the_reference_ceiling_apart_from_the_recorded_deviations() {
    let ceilings: Vec<CeilingCase> = cases(MANIFEST_DIR, "index/eviction", "ceilings");
    for CeilingCase(value, limit, warning, deviation) in ceilings {
        let value = value.as_deref().map(OsStr::from_bytes);

        let found = max_cache_bytes(value);

        if deviation.is_some() {
            // A digit outside ASCII is refused, so the default ceiling applies with the warning.
            assert_eq!(found.0, Some(DEFAULT_MAX_CACHE_BYTES), "{value:?}");
            assert!(found.1.is_some() && warning.is_none(), "{value:?}");
            continue;
        }
        assert_eq!(found, (limit_of(limit.as_deref()), warning), "{value:?}");
    }
}

#[derive(Deserialize)]
struct ReportCase {
    name: String,
    values: ReportValues,
    summary: String,
    json: String,
}

#[derive(Clone, Copy, Deserialize)]
struct ReportValues {
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

fn report_of(case: &ReportCase) -> IndexReport {
    let ReportValues {
        indexed,
        reused,
        pruned,
        files,
        tags,
        imports,
        refs,
        failures,
        skips,
        evicted,
    } = case.values;
    IndexReport {
        database: PathBuf::from(DATABASE),
        generation: "0123abcd".to_owned(),
        indexed,
        reused,
        pruned,
        files,
        tags,
        imports,
        refs,
        failures: (0..failures)
            .map(|i| IndexFailure {
                path: format!("f{i}.py"),
                reason: format!("reason {i}"),
            })
            .collect(),
        skipped_files: (0..skips)
            .map(|i| IndexSkip {
                path: format!("s{i}.py"),
                reason: format!("skip {i}"),
            })
            .collect(),
        evicted: EvictionReport {
            databases: evicted.0,
            size: evicted.1,
        },
    }
}

#[test]
fn each_report_has_the_reference_summary_line_and_json_form() {
    let reports: Vec<ReportCase> = cases(MANIFEST_DIR, "index/report", "reports");
    for case in &reports {
        let report = report_of(case);

        let json = to_text(&report, JsonForm::Compact).expect("the report serializes");

        assert_eq!(report.summary_line(), case.summary, "{}", case.name);
        assert_eq!(json, case.json, "{}", case.name);
    }
}

// state, the note kind or the root, its arguments, refreshing, the receipt
type ReceiptCase = (String, Option<String>, Vec<Value>, bool, String);

fn text(value: &Value) -> String {
    value.as_str().expect("a text argument").to_owned()
}

fn refusal(kind: &str, arguments: &[Value]) -> Refusal {
    let error = |class| SqlError {
        class,
        message: text(&arguments[0]),
    };
    match kind {
        "no_index" => Refusal::NoIndex,
        "schema" => Refusal::Schema {
            found: arguments[0].as_i64().expect("a version"),
            expected: u16::try_from(arguments[1].as_u64().expect("a version")).expect("small"),
        },
        "other_repository" => Refusal::OtherRepository {
            repo_root: text(&arguments[0]),
        },
        "unreadable" => Refusal::Unreadable(error(SqlClass::Operational)),
        "corrupt" => Refusal::Corrupt(error(SqlClass::Database)),
        other => panic!("no note kind {other}"),
    }
}

fn state_of(state: &str, detail: Option<&str>, arguments: &[Value]) -> CacheState {
    match state {
        "bypassed" => CacheState::Bypassed,
        "absent" => CacheState::Absent {
            note: detail.map(|kind| refusal(kind, arguments)),
        },
        "indexed" => CacheState::Indexed {
            generation: text(&arguments[0]),
            repo_generation: text(&arguments[1]),
            repo_root: detail.expect("a repository root").to_owned(),
        },
        other => panic!("no state {other}"),
    }
}

#[test]
fn each_receipt_is_the_reference_cache_field() {
    let receipts: Vec<ReceiptCase> = cases(MANIFEST_DIR, "index/receipts", "receipts");
    for (state, detail, arguments, refreshing, expected) in receipts {
        let status = CacheStatus {
            path: PathBuf::from(DATABASE),
            state: state_of(&state, detail.as_deref(), &arguments),
        };

        let found = status.receipt_while(refreshing);

        assert_eq!(found, expected, "{state} {detail:?} {refreshing}");
    }
}

// database, kind, path, the error class and message, the warning
type RowCase = (String, String, String, String, String, String);

fn os_error(errno: u64) -> std::io::Error {
    std::io::Error::from_raw_os_error(i32::try_from(errno).expect("an errno"))
}

fn section<T: for<'de> Deserialize<'de>>(name: &str) -> Vec<T> {
    cases(MANIFEST_DIR, "index/warnings", name)
}

#[test]
fn each_file_warning_is_the_reference_text() {
    for (path, errno, expected) in section::<(String, u64, String)>("restricting") {
        let found = Failed::Restricting(Path::new(&path)).warning(&os_error(errno));
        assert_eq!(found, expected);
    }
    for (root, errno, expected) in section::<(String, u64, String)>("listing") {
        assert_eq!(
            Failed::Listing(Path::new(&root)).warning(&os_error(errno)),
            expected
        );
    }
    for (database, suffix, errno, expected) in section::<(String, String, u64, String)>("weighing")
    {
        let failed = Failed::Weighing(Path::new(&database), &suffix);
        assert_eq!(failed.warning(&os_error(errno)), expected);
    }
    for (database, errno, expected) in section::<(String, u64, String)>("recheck") {
        assert_eq!(
            Failed::Recheck(Path::new(&database)).warning(&os_error(errno)),
            expected
        );
    }
    for (database, errno, expected) in section::<(String, u64, String)>("eviction") {
        assert_eq!(
            Failed::Eviction(Path::new(&database)).warning(&os_error(errno)),
            expected
        );
    }
}

#[test]
fn the_reclaim_and_row_warnings_are_the_reference_texts() {
    for (class, message, expected) in section::<(String, String, String)>("reclaim") {
        assert_eq!(class, "OperationalError");
        let error = SqlError {
            class: SqlClass::Operational,
            message,
        };
        assert_eq!(unreclaimed(&error), expected);
    }
    for (database, kind, path, class, message, expected) in section::<RowCase>("rows") {
        assert_eq!(class, "DatabaseError");
        let failure = RowFailure {
            database: PathBuf::from(database),
            kind: if kind == "symbol" { "symbol" } else { "block" },
            path,
            cause: SqlError {
                class: SqlClass::Database,
                message,
            }
            .repr(),
        };
        assert_eq!(failure.to_string(), expected);
    }
}
