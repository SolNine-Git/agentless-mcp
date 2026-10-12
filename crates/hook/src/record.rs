//! The one JSON line the gate appends per call, in the field order the report script reads.

use std::time::{SystemTime, UNIX_EPOCH};

use checks::{Detail, Observed, PairFinding, Severity};
use io::GateMode;
use serde::Serialize;

use crate::edit::Eol;
use crate::modes::Modes;

const LOG_VERSION: u8 = 1;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome {
    Clean,
    Advisory,
    Blocked,
    Degraded,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Record {
    pub(crate) v: u8,
    pub(crate) ts: f64,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) session_id: Option<String>,
    pub(crate) transcript_path: Option<String>,
    pub(crate) tool_use_id: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) root: Option<String>,
    pub(crate) rel: Option<String>,
    pub(crate) binary: Option<String>,
    pub(crate) modes: Option<Modes>,
    pub(crate) checks: Vec<&'static str>,
    // In process every gated check is a pair check, so no check is ever dropped.
    pub(crate) dropped: [&'static str; 0],
    pub(crate) off_by_config: Vec<&'static str>,
    pub(crate) config_warnings: Vec<String>,
    pub(crate) old_sha256: Option<String>,
    pub(crate) new_sha256: Option<String>,
    pub(crate) eol: Option<Eol>,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) reason: Option<String>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) findings: Vec<Logged>,
    pub(crate) observed: Option<Observed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Gap {
    pub(crate) check: &'static str,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Logged {
    check: &'static str,
    severity: Severity,
    mode: GateMode,
    path: String,
    line: u32,
    message: String,
    evidence: String,
    detail: Option<Detail>,
}

impl Logged {
    pub(crate) fn of(row: &PairFinding, mode: GateMode) -> Self {
        let finding = &row.finding;
        Self {
            check: finding.check,
            severity: finding.severity,
            mode,
            path: finding.path.clone(),
            line: finding.line,
            message: finding.message.clone(),
            evidence: finding.evidence.clone(),
            detail: row.detail.clone(),
        }
    }
}

// The four payload strings the record copies before the payload's shape is checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) session_id: Option<String>,
    pub(crate) transcript_path: Option<String>,
    pub(crate) tool_use_id: Option<String>,
    pub(crate) tool_name: Option<String>,
}

// The fields the decision fills.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settled {
    pub(crate) outcome: Outcome,
    pub(crate) reason: Option<String>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) off_by_config: Vec<&'static str>,
    pub(crate) findings: Vec<Logged>,
    pub(crate) observed: Option<Observed>,
}

impl Record {
    pub(crate) fn blank(ts: f64) -> Self {
        Self {
            v: LOG_VERSION,
            ts,
            latency_ms: None,
            session_id: None,
            transcript_path: None,
            tool_use_id: None,
            tool_name: None,
            path: None,
            root: None,
            rel: None,
            binary: None,
            modes: None,
            checks: Vec::new(),
            dropped: [],
            off_by_config: Vec::new(),
            config_warnings: Vec::new(),
            old_sha256: None,
            new_sha256: None,
            eol: None,
            outcome: None,
            reason: None,
            gaps: Vec::new(),
            findings: Vec::new(),
            observed: None,
        }
    }

    pub(crate) fn identify(&mut self, identity: Identity) {
        self.session_id = identity.session_id;
        self.transcript_path = identity.transcript_path;
        self.tool_use_id = identity.tool_use_id;
        self.tool_name = identity.tool_name;
    }

    pub(crate) fn settle(&mut self, settled: Settled) {
        self.outcome = Some(settled.outcome);
        self.reason = settled.reason;
        self.gaps = settled.gaps;
        self.off_by_config = settled.off_by_config;
        self.findings = settled.findings;
        self.observed = settled.observed;
    }
}

// Whole seconds stay whole; any other time divides the nanosecond count, as CPython's time.time().
pub(crate) fn seconds(wall: SystemTime) -> f64 {
    let nanos = wall
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    if nanos.is_multiple_of(NANOS_PER_SECOND) {
        whole(nanos / NANOS_PER_SECOND)
    } else {
        whole(nanos) / 1e9
    }
}

// The nearest float to an integer: a decimal parse rounds the way an integer conversion does.
fn whole(count: u128) -> f64 {
    count
        .to_string()
        .parse()
        .expect("a decimal integer parses as a float")
}

pub(crate) fn milliseconds(elapsed: std::time::Duration) -> f64 {
    pycompat::round_half_even(elapsed.as_secs_f64() * 1000.0, 1)
}
