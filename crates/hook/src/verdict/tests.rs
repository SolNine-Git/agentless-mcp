use std::collections::BTreeMap;

use checks::{CheckGap, Detail, Finding, Match, PairFinding, Severity, UnitDelta};
use io::testing::cases;
use io::{GateMode, GatedCheck};
use json::{JsonForm, to_text};
use serde_json::Value;

use super::classify;
use crate::modes::{Modes, resolve};
use crate::testing::MANIFEST_DIR;

fn modes(given: &Value) -> Modes {
    let mode = |name: &str| {
        GateMode::ALL
            .into_iter()
            .find(|mode| mode.name() == name)
            .unwrap_or_else(|| panic!("no mode {name}"))
    };
    let entries: BTreeMap<GatedCheck, GateMode> = GatedCheck::ALL
        .into_iter()
        .map(|check| (check, mode(given[check.name()].as_str().expect("a mode"))))
        .collect();
    resolve(Some(&entries), None).0
}

fn check(name: &str) -> &'static str {
    [
        checks::COMPLEXITY_DELTA,
        checks::CONTRACT_DRIFT,
        checks::CLONE_DELTA,
        app::COVERAGE,
    ]
    .into_iter()
    .find(|check| *check == name)
    .unwrap_or_else(|| panic!("no check {name}"))
}

fn severity(name: &str) -> Severity {
    [Severity::Advisory, Severity::Warning, Severity::NotChecked]
        .into_iter()
        .find(|severity| severity.as_str() == name)
        .unwrap_or_else(|| panic!("no severity {name}"))
}

// The gap of the pair-check language with this name, found through a file name that selects it.
fn no_rules(name: &str) -> CheckGap {
    [
        "x.py", "x.js", "x.ts", "x.tsx", "x.go", "x.rs", "x.java", "x.c", "x.cpp", "x.rb",
    ]
    .into_iter()
    .filter_map(checks::pair_language)
    .find(|language| language.name() == name)
    .map_or_else(
        || panic!("no pair-check language {name}"),
        CheckGap::NoRulesForLanguage,
    )
}

fn number(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .map(|n| u32::try_from(n).expect("a small number"))
}

fn detail(value: &Value) -> Option<Detail> {
    let text = |key: &str| value[key].as_str().expect(key).to_owned();
    match value {
        Value::Null => None,
        _ if value.get("reason").is_some() => Some(Detail::Gap(match text("reason").as_str() {
            "no_boundary_globs" => CheckGap::NoBoundaryGlobs,
            "no_tag_cache" => CheckGap::NoTagCache,
            _ => no_rules(&text("language")),
        })),
        _ => Some(Detail::Unit(UnitDelta {
            unit: text("unit"),
            via: [Match::Body, Match::Name, Match::Collision, Match::New]
                .into_iter()
                .find(|via| serde_json::to_value(via).ok() == Some(value["match"].clone()))
                .expect("a match name"),
            cc_old: number(&value["cc_old"]),
            cc_new: number(&value["cc_new"]).expect("cc_new"),
            sloc_old: number(&value["sloc_old"]),
            sloc_new: number(&value["sloc_new"]).expect("sloc_new"),
            threshold: number(&value["threshold"]).expect("threshold"),
        })),
    }
}

fn row(value: &Value) -> PairFinding {
    let text = |key: &str| value[key].as_str().expect(key).to_owned();
    PairFinding {
        finding: Finding {
            check: check(&text("check")),
            severity: severity(&text("severity")),
            message: text("message"),
            path: text("path"),
            line: number(&value["line"]).expect("a line"),
            evidence: text("evidence"),
        },
        detail: detail(value.get("detail").unwrap_or(&Value::Null)),
    }
}

#[test]
fn the_verdict_and_the_reply_match_the_generated_cases() {
    let all: Vec<(String, Value, Vec<Value>, Value)> = cases(MANIFEST_DIR, "classify", "classify");
    assert!(!all.is_empty());
    for (name, given_modes, given_rows, expected) in all {
        let modes = modes(&given_modes);
        let rows: Vec<PairFinding> = given_rows.iter().map(row).collect();
        let verdict = classify(&rows, modes).expect("each generated row names a gated check");
        let decision = verdict.decision(modes).expect("the reply serializes");
        assert_eq!(Value::from(decision.code), expected["code"], "case {name}");
        assert_eq!(decision.stdout, expected["stdout"], "case {name}");
        assert_eq!(decision.stderr, expected["stderr"], "case {name}");
        let settled = verdict.settled(None);
        let outcome = serde_json::to_value(settled.outcome).expect("an outcome name");
        assert_eq!(outcome, expected["outcome"], "case {name}");
        assert_eq!(
            Value::from(settled.reason),
            expected["reason"],
            "case {name}"
        );
        assert_eq!(shown(&settled.gaps), expected["gaps"], "case {name}");
        assert_eq!(
            shown(&settled.findings),
            expected["findings"],
            "case {name}"
        );
        assert_eq!(
            Value::from(settled.off_by_config),
            expected["off_by_config"],
            "case {name}"
        );
    }
}

// The compact JSON text of a value, as CPython's json.dumps writes it.
fn shown<T: serde::Serialize>(value: &T) -> String {
    to_text(value, JsonForm::Compact).expect("the value serializes")
}

#[test]
fn a_row_for_a_check_that_is_not_gated_is_an_internal_error() {
    let finding = Finding {
        check: app::COVERAGE,
        severity: Severity::Warning,
        message: "m".to_owned(),
        path: "a.py".to_owned(),
        line: 1,
        evidence: String::new(),
    };
    let rows = [PairFinding {
        finding,
        detail: None,
    }];
    let modes = resolve(None, None).0;
    let halt = classify(&rows, modes).expect_err("coverage has no mode");
    assert_eq!(
        halt.reason,
        "internal error: lint returned a row for coverage"
    );
}
