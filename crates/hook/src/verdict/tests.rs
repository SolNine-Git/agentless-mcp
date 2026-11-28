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
fn the_verdict_and_the_reply_match_the_reference_cases() {
    let all: Vec<(String, Value, Vec<Value>, Value)> = cases(MANIFEST_DIR, "classify", "classify");
    assert!(!all.is_empty());
    for (name, given_modes, given_rows, expected) in all {
        let modes = modes(&given_modes);
        let rows: Vec<PairFinding> = given_rows.iter().map(row).collect();
        let verdict = classify(&rows, modes).expect("each reference row names a gated check");
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

fn warning_at(check: &'static str, line: u32) -> PairFinding {
    PairFinding {
        finding: Finding {
            check,
            severity: Severity::Warning,
            message: format!("{check} at line {line}"),
            path: "a.py".to_owned(),
            line,
            evidence: String::new(),
        },
        detail: None,
    }
}

#[test]
fn a_fix_line_that_two_blocking_checks_share_prints_once() {
    let rows = [
        warning_at(checks::COMPLEXITY_DELTA, 1),
        warning_at(checks::CLONE_DELTA, 2),
    ];
    let modes = resolve(None, Some(std::ffi::OsStr::new("block"))).0;
    let verdict = classify(&rows, modes).expect("both checks are gated");
    let decision = verdict.decision(modes).expect("the reply serializes");
    assert_eq!(
        decision.stderr,
        "Erosion gate blocked this edit:\n\
         a.py:1: complexity_delta at line 1\n\
         a.py:2: clone_delta at line 2\n\
         Change the code as each blocking finding says, then retry the edit.\n"
    );
}

// comment_delta blocks by default, so its rules close the text, and an advisory finding stays.
#[test]
fn a_comment_block_ends_with_the_comment_rules_and_no_mode_line() {
    let rows = [
        warning_at(checks::COMMENT_DELTA, 1),
        warning_at(checks::COMPLEXITY_DELTA, 2),
    ];
    let modes = resolve(None, None).0;
    let verdict = classify(&rows, modes).expect("both checks are gated");
    let decision = verdict.decision(modes).expect("the reply serializes");
    assert_eq!(decision.code, 2);
    assert_eq!(
        decision.stderr,
        "Erosion gate blocked this edit:\n\
         a.py:1: comment_delta at line 1\n\
         Advisory findings (these do not block):\n\
         a.py:2: complexity_delta at line 2\n\
         - Docstrings on public API only; none on _private or dunder functions, Rust items \
         without pub, or trait impls.\n\
         - No Args/Returns/Raises sections, JSDoc @param/@returns tags or Rust # Arguments/# \
         Returns sections; the typed signature carries them.\n\
         - No banner comments. Why-comments run 1-2 lines.\n\
         - Never silence a linter, formatter or type checker; fix the code it flags.\n\
         Delete the flagged text, then retry the edit. Do not reword it.\n"
    );
}

#[test]
fn a_language_with_no_comment_rules_is_off_by_config() {
    let go = checks::pair_language("x.go").expect("go is a pair-check language");
    let gap = CheckGap::NoCommentRules(go, prompts::CommentGroup::Docs);
    let mut row = warning_at(checks::COMMENT_DELTA, 0);
    row.finding.severity = Severity::NotChecked;
    row.detail = Some(Detail::Gap(gap));
    let modes = resolve(None, None).0;
    let verdict = classify(std::slice::from_ref(&row), modes).expect("a gap row");
    let settled = verdict.settled(None);
    assert_eq!(settled.off_by_config, [checks::COMMENT_DELTA]);
    assert!(settled.gaps.is_empty());
    assert_eq!(
        serde_json::to_value(settled.outcome).ok(),
        Some("clean".into())
    );
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
