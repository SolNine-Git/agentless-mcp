//! The one JSON line the gate appends per call, in the field order the report reads.

use std::time::{SystemTime, UNIX_EPOCH};

use checks::{Detail, Observed, PairFinding, Severity};
use io::GateMode;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::edit::Eol;
use crate::modes::Modes;

pub(crate) const LOG_VERSION: u8 = 1;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

// The key of each field of a record and of a logged finding. Logs from earlier builds use the
// same keys, and the report reads every log by them.
pub(crate) mod key {
    pub(crate) const V: &str = "v";
    pub(crate) const TS: &str = "ts";
    pub(crate) const LATENCY_MS: &str = "latency_ms";
    pub(crate) const SESSION_ID: &str = "session_id";
    pub(crate) const TRANSCRIPT_PATH: &str = "transcript_path";
    pub(crate) const TOOL_USE_ID: &str = "tool_use_id";
    pub(crate) const TOOL_NAME: &str = "tool_name";
    pub(crate) const PATH: &str = "path";
    pub(crate) const ROOT: &str = "root";
    pub(crate) const REL: &str = "rel";
    pub(crate) const BINARY: &str = "binary";
    pub(crate) const MODES: &str = "modes";
    pub(crate) const CHECKS: &str = "checks";
    pub(crate) const DROPPED: &str = "dropped";
    pub(crate) const OFF_BY_CONFIG: &str = "off_by_config";
    pub(crate) const CONFIG_WARNINGS: &str = "config_warnings";
    pub(crate) const OLD_SHA256: &str = "old_sha256";
    pub(crate) const NEW_SHA256: &str = "new_sha256";
    pub(crate) const EOL: &str = "eol";
    pub(crate) const OUTCOME: &str = "outcome";
    pub(crate) const REASON: &str = "reason";
    pub(crate) const GAPS: &str = "gaps";
    pub(crate) const FINDINGS: &str = "findings";
    pub(crate) const OBSERVED: &str = "observed";
    pub(crate) const CHECK: &str = "check";
    pub(crate) const SEVERITY: &str = "severity";
    pub(crate) const MODE: &str = "mode";
    pub(crate) const LINE: &str = "line";
    pub(crate) const MESSAGE: &str = "message";
    pub(crate) const EVIDENCE: &str = "evidence";
    pub(crate) const DETAIL: &str = "detail";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Clean,
    Advisory,
    Blocked,
    Degraded,
    Skipped,
}

impl Outcome {
    // Every outcome, in the order the report counts them.
    pub(crate) const ALL: [Outcome; 5] = [
        Self::Clean,
        Self::Advisory,
        Self::Blocked,
        Self::Degraded,
        Self::Skipped,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Advisory => "advisory",
            Self::Blocked => "blocked",
            Self::Degraded => "degraded",
            Self::Skipped => "skipped",
        }
    }
}

impl Serialize for Outcome {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

// Serializes each named struct as one JSON object: each field under its key const, in order.
macro_rules! keyed {
    ($($name:ident { $($key:ident: $field:ident),+ $(,)? })+) => {$(
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let fields = [$(stringify!($field)),+].len();
                let mut row = serializer.serialize_struct(stringify!($name), fields)?;
                $(row.serialize_field(key::$key, &self.$field)?;)+
                row.end()
            }
        }
    )+};
}

keyed! {
    Record {
        V: v,
        TS: ts,
        LATENCY_MS: latency_ms,
        SESSION_ID: session_id,
        TRANSCRIPT_PATH: transcript_path,
        TOOL_USE_ID: tool_use_id,
        TOOL_NAME: tool_name,
        PATH: path,
        ROOT: root,
        REL: rel,
        BINARY: binary,
        MODES: modes,
        CHECKS: checks,
        DROPPED: dropped,
        OFF_BY_CONFIG: off_by_config,
        CONFIG_WARNINGS: config_warnings,
        OLD_SHA256: old_sha256,
        NEW_SHA256: new_sha256,
        EOL: eol,
        OUTCOME: outcome,
        REASON: reason,
        GAPS: gaps,
        FINDINGS: findings,
        OBSERVED: observed,
    }
    Gap { CHECK: check, MESSAGE: message }
    Logged {
        CHECK: check,
        SEVERITY: severity,
        MODE: mode,
        PATH: path,
        LINE: line,
        MESSAGE: message,
        EVIDENCE: evidence,
        DETAIL: detail,
    }
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    pub(crate) check: &'static str,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

// Whole seconds stay whole. Any other time divides the nanosecond count by 1e9.
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
    json::round_half_even(elapsed.as_secs_f64() * 1000.0, 1)
}
