//! The gate log read into records: each line one JSON object, every field checked in the order
//! the reference checks it, so a malformed line is refused with the same text.

use std::fs;
use std::path::Path;

use ::text::CodePointString;
use checks::{Match, Severity, schema};
use io::GatedCheck;
use json::{JsonErrorKind, JsonValue};

use crate::record::{LOG_VERSION, Outcome, key};
use crate::report::field::{self, Int, Kind, Pairs};
use crate::text::codec_text;

const FINDING_OUTCOMES: [Outcome; 2] = [Outcome::Advisory, Outcome::Blocked];
const FINDING_SEVERITIES: [Severity; 2] = [Severity::Warning, Severity::Advisory];

// One hook call from the log; `line_no` is its line in the log.
pub(crate) struct Record {
    pub(crate) line_no: usize,
    pub(crate) ts: f64,
    pub(crate) session_id: Option<CodePointString>,
    pub(crate) transcript_path: Option<CodePointString>,
    pub(crate) tool_use_id: Option<CodePointString>,
    pub(crate) path: Option<CodePointString>,
    pub(crate) outcome: Outcome,
    pub(crate) reason: Option<CodePointString>,
    pub(crate) binary: Option<CodePointString>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) observed: Option<Observed>,
}

// One logged finding, with the numbers or the key that its check tracks it by.
pub(crate) struct Finding {
    pub(crate) check: CodePointString,
    pub(crate) path: CodePointString,
    pub(crate) line: Int,
    pub(crate) message: CodePointString,
    pub(crate) evidence: CodePointString,
    pub(crate) unit: Option<CodePointString>,
    pub(crate) via: Option<Match>,
    pub(crate) tracked: Option<Tracked>,
}

pub(crate) enum Tracked {
    Complexity {
        cc_old: Option<Int>,
        threshold: Int,
    },
    Contract {
        rule: CodePointString,
        capture: CodePointString,
    },
    Clone {
        key: CodePointString,
    },
}

// The observed state of a record: `None` for a list whose check measured no file.
pub(crate) struct Observed {
    pub(crate) units_over_threshold: Option<Vec<UnitCc>>,
    pub(crate) contract_keys: Option<Vec<Hit>>,
    pub(crate) clone_instances: Option<Vec<Instance>>,
}

pub(crate) struct UnitCc {
    pub(crate) path: CodePointString,
    pub(crate) unit: CodePointString,
    pub(crate) cc: Int,
}

pub(crate) struct Hit {
    pub(crate) path: CodePointString,
    pub(crate) rule: CodePointString,
    pub(crate) capture: CodePointString,
}

pub(crate) struct Instance {
    pub(crate) path: CodePointString,
    pub(crate) key: CodePointString,
}

// The check whose name `check` is, when it is a check the report tracks.
pub(crate) fn gated(check: &CodePointString) -> Option<GatedCheck> {
    GatedCheck::ALL
        .into_iter()
        .find(|gated| check.as_str() == Some(gated.name()))
}

pub(crate) fn read_log(path: &str) -> Result<Vec<Record>, String> {
    let bytes = fs::read(path).map_err(|error| {
        let shown = io::os_error_text(&error, Path::new(path));
        format!("cannot read {path}: {shown}")
    })?;
    let text = decoded(bytes).map_err(|error| format!("cannot read {path}: {error}"))?;
    let mut records = Vec::new();
    for (index, line) in ::text::splitlines(&text, false).into_iter().enumerate() {
        if ::text::strip(line).is_empty() {
            continue;
        }
        let line_no = index + 1;
        let at = format!("{path}:{line_no}");
        let document = parsed(line, &at)?;
        records.push(record(&document, line_no, &at)?);
    }
    Ok(records)
}

// The text of `bytes`, or the reference's decode error text. Only refused bytes are copied.
pub(crate) fn decoded(bytes: Vec<u8>) -> Result<String, String> {
    let refused = if std::str::from_utf8(&bytes).is_err() {
        bytes.clone()
    } else {
        Vec::new()
    };
    io::decode_strict(bytes).map_err(|error| codec_text(&refused, error))
}

// Nesting past the depth limit ends the reference in a RecursionError, whose last traceback line
// is the message (ledger deviation).
fn parsed(line: &str, at: &str) -> Result<JsonValue, String> {
    json::parse(line).map_err(|error| match error.kind {
        JsonErrorKind::ObjectTooDeep | JsonErrorKind::ArrayTooDeep => {
            format!("RecursionError: {}", error.kind)
        }
        _ => format!("{at}: not JSON: {error}"),
    })
}

fn record(document: &JsonValue, line_no: usize, at: &str) -> Result<Record, String> {
    let Some(object) = field::seen(document).dict else {
        return Err(format!("{at}: not a JSON object"));
    };
    let version = field::int(object, key::V, at)?;
    if version.to_string() != LOG_VERSION.to_string() {
        return Err(format!("{at}: log version {version}, not {LOG_VERSION}"));
    }
    let outcome = outcome(object, at)?;
    let observed = match field::dict_or_null(object, key::OBSERVED, at)? {
        Some(value) => Some(observed(value, at)?),
        None => None,
    };
    // A record that carries findings or an observed state must say whose call it was.
    let named = FINDING_OUTCOMES.contains(&outcome) || observed.is_some();
    let findings = field::list(object, key::FINDINGS, at)?;
    let ts = ts(object, at)?;
    let [session_id, transcript_path, tool_use_id, path] = identity(object, at, named)?;
    Ok(Record {
        line_no,
        ts,
        session_id,
        transcript_path,
        tool_use_id,
        path,
        outcome,
        reason: field::text_or_null(object, key::REASON, at)?,
        binary: field::text_or_null(object, key::BINARY, at)?,
        findings: logged(findings, at)?,
        observed,
    })
}

fn outcome(object: &Pairs, at: &str) -> Result<Outcome, String> {
    let name = field::text(object, key::OUTCOME, at)?;
    let found = Outcome::ALL
        .into_iter()
        .find(|outcome| name.as_str() == Some(outcome.name()));
    found.ok_or_else(|| {
        let names: Vec<&str> = Outcome::ALL.iter().map(|outcome| outcome.name()).collect();
        let shown = name.quoted();
        format!("{at}: outcome {shown} is not one of {}", names.join(", "))
    })
}

// An int that is too large for a float ends the reference in an OverflowError (ledger deviation).
fn ts(object: &Pairs, at: &str) -> Result<f64, String> {
    let read = |value| {
        let seen = field::seen(value);
        match value {
            JsonValue::Float(x) => Some(Ok(*x)),
            _ => seen.int.map(|int| int.to_float().ok_or(())),
        }
    };
    let ts = field::typed(object, key::TS, &[Kind::Int, Kind::Float], at, read)?;
    ts.map_err(|()| "OverflowError: int too large to convert to float".to_owned())
}

// The session, the transcript, the call and the path: text when `named`, else text or null.
fn identity(object: &Pairs, at: &str, named: bool) -> Result<[Option<CodePointString>; 4], String> {
    let read = |name| {
        if named {
            field::text(object, name, at).map(Some)
        } else {
            field::text_or_null(object, name, at)
        }
    };
    Ok([
        read(key::SESSION_ID)?,
        read(key::TRANSCRIPT_PATH)?,
        read(key::TOOL_USE_ID)?,
        read(key::PATH)?,
    ])
}

fn logged(items: &[JsonValue], at: &str) -> Result<Vec<Finding>, String> {
    let read = |(index, item)| finding(item, &format!("{at}: findings[{index}]"));
    items.iter().enumerate().map(read).collect()
}

fn finding(item: &JsonValue, at: &str) -> Result<Finding, String> {
    let Some(object) = field::seen(item).dict else {
        return Err(format!("{at} is not an object"));
    };
    let check = field::text(object, key::CHECK, at)?;
    let severity = field::text(object, key::SEVERITY, at)?;
    let known = FINDING_SEVERITIES.map(Severity::as_str);
    if !known.iter().any(|name| severity.as_str() == Some(name)) {
        let shown = severity.quoted();
        return Err(format!("{at}: severity {shown} is not warning or advisory"));
    }
    let finding = Finding {
        check,
        path: field::text(object, key::PATH, at)?,
        line: field::int(object, key::LINE, at)?,
        message: field::text(object, key::MESSAGE, at)?,
        evidence: field::text(object, key::EVIDENCE, at)?,
        unit: None,
        via: None,
        tracked: None,
    };
    let detail = field::dict_or_null(object, key::DETAIL, at)?;
    let Some(rule) = gated(&finding.check) else {
        return Ok(finding);
    };
    let Some(detail) = detail else {
        let check = rule.name();
        return Err(format!(
            "{at}: detail is missing, and a {check} finding needs it"
        ));
    };
    with_detail(finding, rule, detail, &format!("{at}.detail"))
}

fn with_detail(
    finding: Finding,
    rule: GatedCheck,
    detail: &Pairs,
    at: &str,
) -> Result<Finding, String> {
    match rule {
        GatedCheck::ComplexityDelta => with_numbers(finding, detail, at),
        GatedCheck::ContractDrift => with_key(finding, detail, at),
        GatedCheck::CloneDelta => with_clone(finding, detail, at),
    }
}

// A hit is compared without its unit: a rename alone must not read as a fix.
fn with_key(finding: Finding, detail: &Pairs, at: &str) -> Result<Finding, String> {
    let rule = field::text(detail, schema::RULE, at)?;
    let unit = field::text(detail, schema::UNIT, at)?;
    let capture = field::text(detail, schema::CAPTURE, at)?;
    Ok(Finding {
        unit: Some(unit),
        tracked: Some(Tracked::Contract { rule, capture }),
        ..finding
    })
}

fn with_numbers(finding: Finding, detail: &Pairs, at: &str) -> Result<Finding, String> {
    let via = matched(detail, at)?;
    field::int(detail, schema::CC_NEW, at)?;
    field::int(detail, schema::SLOC_NEW, at)?;
    field::int_or_null(detail, schema::SLOC_OLD, at)?;
    let unit = field::text(detail, schema::UNIT, at)?;
    let cc_old = field::int_or_null(detail, schema::CC_OLD, at)?;
    let threshold = field::int(detail, schema::THRESHOLD, at)?;
    Ok(Finding {
        unit: Some(unit),
        via: Some(via),
        tracked: Some(Tracked::Complexity { cc_old, threshold }),
        ..finding
    })
}

fn with_clone(finding: Finding, detail: &Pairs, at: &str) -> Result<Finding, String> {
    let key = field::text(detail, schema::KEY, at)?;
    field::int(detail, schema::LINE, at)?;
    field::int(detail, schema::END_LINE, at)?;
    field::int(detail, schema::SITES_TOTAL, at)?;
    let sites = field::list(detail, schema::SITES, at)?;
    for (index, site) in sites.iter().enumerate() {
        let at = format!("{at}.sites[{index}]");
        let Some(site) = field::seen(site).dict else {
            return Err(format!("{at} is not an object"));
        };
        field::text(site, schema::PATH, &at)?;
        field::int(site, schema::LINE, &at)?;
        field::int(site, schema::END_LINE, &at)?;
    }
    Ok(Finding {
        tracked: Some(Tracked::Clone { key }),
        ..finding
    })
}

fn matched(row: &Pairs, at: &str) -> Result<Match, String> {
    let name = field::text(row, schema::MATCH, at)?;
    let found = Match::ALL
        .into_iter()
        .find(|pass| name.as_str() == Some(pass.as_str()));
    found.ok_or_else(|| {
        let names: Vec<&str> = Match::ALL.iter().map(|pass| pass.as_str()).collect();
        let shown = name.quoted();
        format!("{at}: match {shown} is not one of {}", names.join(", "))
    })
}

fn observed(value: &Pairs, at: &str) -> Result<Observed, String> {
    let units_over_threshold = rows(value, schema::UNITS_OVER_THRESHOLD, at, unit_cc)?;
    rows(value, schema::UNITS_CHANGED, at, unit_change)?;
    Ok(Observed {
        units_over_threshold,
        contract_keys: rows(value, schema::CONTRACT_KEYS, at, hit)?,
        clone_instances: rows(value, schema::CLONE_INSTANCES, at, instance)?,
    })
}

// An absent list means that its check measured no file, which is not an empty result.
fn rows<T>(
    value: &Pairs,
    key: &str,
    at: &str,
    parse: fn(&Pairs, &str) -> Result<T, String>,
) -> Result<Option<Vec<T>>, String> {
    if json::get(value, key).is_none() {
        return Ok(None);
    }
    let mut parsed = Vec::new();
    for (index, row) in field::list(value, key, at)?.iter().enumerate() {
        let at = format!("{at}: observed.{key}[{index}]");
        let Some(row) = field::seen(row).dict else {
            return Err(format!("{at} is not an object"));
        };
        parsed.push(parse(row, &at)?);
    }
    Ok(Some(parsed))
}

fn unit_cc(row: &Pairs, at: &str) -> Result<UnitCc, String> {
    let path = field::text(row, schema::PATH, at)?;
    let unit = field::text(row, schema::UNIT, at)?;
    field::int(row, schema::LINE, at)?;
    let cc = field::int(row, schema::CC, at)?;
    field::int(row, schema::SLOC, at)?;
    Ok(UnitCc { path, unit, cc })
}

fn unit_change(row: &Pairs, at: &str) -> Result<(), String> {
    matched(row, at)?;
    field::text(row, schema::PATH, at)?;
    field::text(row, schema::UNIT, at)?;
    field::int(row, schema::LINE, at)?;
    field::int_or_null(row, schema::CC_OLD, at)?;
    field::int(row, schema::CC_NEW, at)?;
    field::int_or_null(row, schema::SLOC_OLD, at)?;
    field::int(row, schema::SLOC_NEW, at)?;
    Ok(())
}

fn hit(row: &Pairs, at: &str) -> Result<Hit, String> {
    let path = field::text(row, schema::PATH, at)?;
    let rule = field::text(row, schema::RULE, at)?;
    field::text(row, schema::UNIT, at)?;
    let capture = field::text(row, schema::CAPTURE, at)?;
    Ok(Hit {
        path,
        rule,
        capture,
    })
}

fn instance(row: &Pairs, at: &str) -> Result<Instance, String> {
    let path = field::text(row, schema::PATH, at)?;
    let key = field::text(row, schema::KEY, at)?;
    field::int(row, schema::LINE, at)?;
    field::int(row, schema::END_LINE, at)?;
    Ok(Instance { path, key })
}
