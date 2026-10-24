use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a text: {value}"))
}

fn env_for(run: &Value) -> EnvConfig {
    let columns = run["columns"]
        .as_str()
        .map(|width| (OsString::from("COLUMNS"), OsString::from(width)));
    EnvConfig::new(columns, PathBuf::from("/"))
}

fn parsed(args: &GateReportArgs) -> Value {
    json!({"log": args.log, "sample_out": args.sample_out, "labels": args.labels, "seed": args.seed})
}

// A digit outside ASCII is refused as an int is refused (ledger deviation), so its case holds
// the text of that refusal; the reference's own value stays beside it.
#[test]
fn gate_report_parser_cases() {
    let runs: Vec<Value> =
        io::testing::cases(env!("CARGO_MANIFEST_DIR"), "gate_report/parser", "runs");
    assert!(runs.len() > 30, "{} runs", runs.len());
    for run in runs {
        let argv: Vec<OsString> = run["argv"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|arg| OsString::from(text(arg)))
            .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        let code = match gate_report_args(&argv, &env_for(&run), &mut streams) {
            Ok(request) => {
                assert_eq!(parsed(&request), run["parsed"], "{argv:?}");
                0
            }
            Err(code) => code,
        };
        assert_eq!(Some(u64::from(code)), run["code"].as_u64(), "{argv:?}");
        assert_eq!(String::from_utf8_lossy(&out), text(&run["out"]), "{argv:?}");
        assert_eq!(String::from_utf8_lossy(&err), text(&run["err"]), "{argv:?}");
    }
}
