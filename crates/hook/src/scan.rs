//! The erosion-scan hook: the writes that skip the Write and Edit tools. A shell write marks the
//! repositories it touched and reports each new finding of the checks in block mode once. A Stop
//! or `SubagentStop` scans every marked repository and reports every finding still on disk.

mod markers;
mod payload;
mod roots;

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use app::hooks::Hook;
use app::{
    EnvConfig, LintForm, LintReport, LintRequest, PairCheck, PairSelection, Repo, RevisionLimits,
    Services,
};
use checks::{Detail, Finding, PairFinding, Severity};
use json::JsonValue;
use kernel::Clock;
use prompts::messages::{EROSION_SCAN_STOP_CLOSING, EROSION_SCAN_WRITE_CLOSING};

use crate::halt::Halt;
use crate::search::log::{self, Entry, key};
use crate::search::write;
use crate::text::{panic_text, shown};
use crate::worker::{self, Finished};
use crate::{ENV_GATE, OFF, Output, modes, payload as erosion_payload, verdict};
use markers::Markers;
use payload::{Action, Payload};

const ENV_LOG: &str = "AGENTLESS_EROSION_SCAN_LOG";
const ENV_IGNORE: &str = "AGENTLESS_EROSION_SCAN_IGNORE";
const THREAD: &str = "erosion-scan";
const BLOCK_EXIT: u8 = 2;
const NO_BASE: &str = "HEAD";
// The same bounds as the Python scan that this hook replaces.
const LIMITS: RevisionLimits = RevisionLimits {
    max_files: 300,
    max_bytes: 400_000,
};

const HOOK_KEY: &str = "hook";
const ROOTS_KEY: &str = "roots";
const FINDINGS_KEY: &str = "findings";
const ERROR_KEY: &str = "error";
const MS_KEY: &str = "ms";

// What the scan reads from its process.
struct Settings {
    state: PathBuf,
    ignore: Vec<PathBuf>,
    home: Option<PathBuf>,
    env_gate: Option<OsString>,
}

impl Settings {
    fn of(env: &EnvConfig) -> Self {
        let home = env.var("HOME").map(PathBuf::from);
        Self {
            state: markers::base(env),
            ignore: roots::ignored(env.var(ENV_IGNORE), home.as_deref()),
            home,
            env_gate: env.var(ENV_GATE).map(OsStr::to_os_string),
        }
    }
}

// What a run did: why it stopped or what it found, and the text for the model, if any.
struct Report {
    reason: String,
    roots: usize,
    findings: usize,
    error: Option<String>,
    text: Option<String>,
}

impl Report {
    fn quiet(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            roots: 0,
            findings: 0,
            error: None,
            text: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::quiet("error")
        }
    }

    fn halted(halt: &Halt) -> Self {
        Self::quiet(halt.reason.clone())
    }
}

// One finding, with the repository that holds it and the key that a second report matches.
struct Hit {
    root: PathBuf,
    finding: Finding,
    key: String,
}

impl Hit {
    fn absolute(&self) -> String {
        shown(&self.root.join(&self.finding.path))
    }

    fn named(&self) -> Finding {
        let name = self
            .root
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        Finding {
            path: format!("{name}/{}", self.finding.path),
            ..self.finding.clone()
        }
    }
}

// The roots to scan, and why git could not name some root or a mark could not be kept.
struct Targets {
    roots: Vec<PathBuf>,
    error: Option<String>,
}

// Scans for the one payload on `stdin`. Fail open: every error and panic exits 0 and is logged.
pub(crate) fn run(
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
    clock: &dyn Clock,
) -> Output {
    if env.var(ENV_GATE) == Some(OsStr::new(OFF)) {
        return Output::default();
    }
    let started = clock.now();
    let scanned = panic::catch_unwind(AssertUnwindSafe(|| scanned(env, services, stdin)));
    let (payload, report) = scanned.unwrap_or_else(|panicked| {
        let message = panic_text(&*panicked);
        (None, Report::failed(format!("panic: {message}")))
    });
    let ms = clock.now().saturating_duration_since(started).as_millis();
    let entry = entry(payload.as_ref(), &report, log::now(clock), ms);
    // The log is the hook's only channel: a failed append has nowhere to be reported.
    drop(log::append(env.var(ENV_LOG), &entry));
    output(report)
}

fn output(report: Report) -> Output {
    match report.text {
        Some(text) => Output {
            code: BLOCK_EXIT,
            stderr: text,
            ..Output::default()
        },
        None => Output::default(),
    }
}

fn scanned(
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
) -> (Option<Payload>, Report) {
    let parsed = erosion_payload::read(stdin).and_then(|raw| payload::parse(&raw));
    match parsed {
        Ok(payload) => {
            let report = decided(env, services, &payload);
            (Some(payload), report)
        }
        Err(halt) => (None, Report::halted(&halt)),
    }
}

fn decided(env: &EnvConfig, services: &Services, payload: &Payload) -> Report {
    if payload.action == (Action::Stop { active: true }) {
        return Report::quiet("stop_hook_active");
    }
    let (services, payload, settings) = (services.clone(), payload.clone(), Settings::of(env));
    let work = move || Ok(inspect(&services, &settings, &payload));
    match worker::run(THREAD, work, Hook::ErosionScan.deadline()) {
        Ok(Finished::Done(report)) => report,
        Ok(Finished::TimedOut) => Report::quiet("deadline"),
        Ok(Finished::Failed(error)) => Report::failed(error.to_string()),
        Ok(Finished::Panicked(message)) => Report::failed(format!("panic: {message}")),
        Err(error) => Report::failed(format!("the scan thread did not start: {error}")),
    }
}

fn inspect(services: &Services, settings: &Settings, payload: &Payload) -> Report {
    let markers = Markers::new(&settings.state, &payload.session);
    let targets = match targets(services, settings, &markers, payload) {
        Ok(targets) => targets,
        Err(reason) => return Report::quiet(reason),
    };
    let (hits, errors) = scanned_roots(services, settings, &targets.roots);
    let hits = match payload.action {
        Action::Shell(_) => fresh(&markers, hits),
        Action::Stop { .. } => hits,
    };
    let error =
        (targets.error.into_iter().chain(errors)).reduce(|all, next| format!("{all}; {next}"));
    Report {
        reason: reason(&hits, error.as_deref()).to_owned(),
        roots: targets.roots.len(),
        findings: hits.len(),
        error,
        text: text(&hits, &payload.action),
    }
}

// A scan that failed on any root is degraded, never clean.
fn reason(hits: &[Hit], error: Option<&str>) -> &'static str {
    match (hits.is_empty(), error) {
        (false, _) => "reported",
        (true, Some(_)) => "degraded",
        (true, None) => "clean",
    }
}

// A shell write marks and scans the roots that it names; a Stop scans every marked root.
fn targets(
    services: &Services,
    settings: &Settings,
    markers: &Markers,
    payload: &Payload,
) -> Result<Targets, &'static str> {
    let targets = match &payload.action {
        Action::Shell(command) => written(services, settings, markers, &payload.cwd, command)?,
        Action::Stop { .. } => Targets {
            roots: unignored(markers.marked(), &settings.ignore),
            error: None,
        },
    };
    if targets.roots.is_empty() {
        return Err("no_roots");
    }
    Ok(targets)
}

fn written(
    services: &Services,
    settings: &Settings,
    markers: &Markers,
    cwd: &Path,
    command: &str,
) -> Result<Targets, &'static str> {
    if !write::writes(command) {
        return Err("not_a_write");
    }
    let found = roots::of_command(services, settings.home.as_deref(), cwd, command);
    let roots = unignored(found.roots, &settings.ignore);
    let unmarked = roots.iter().find(|root| !markers.mark(root));
    let unmarked = unmarked.map(|root| format!("cannot mark {}", shown(root)));
    Ok(Targets {
        error: found.error.or(unmarked),
        roots,
    })
}

fn unignored(mut roots: Vec<PathBuf>, ignore: &[PathBuf]) -> Vec<PathBuf> {
    roots.retain(|root| !ignore.contains(root));
    roots
}

// The findings of every root, and the error of each root that could not be scanned.
fn scanned_roots(
    services: &Services,
    settings: &Settings,
    roots: &[PathBuf],
) -> (Vec<Hit>, Vec<String>) {
    let (mut hits, mut errors) = (Vec::new(), Vec::new());
    for root in roots {
        let gate = settings.env_gate.as_deref();
        match scan_root(services, gate, root) {
            Ok(found) => hits.extend(found),
            Err(error) => errors.push(format!("{}: {error}", shown(root))),
        }
    }
    (hits, errors)
}

// The findings that the checks in block mode report for the files changed since the closest base.
fn scan_root(
    services: &Services,
    env_gate: Option<&OsStr>,
    root: &Path,
) -> Result<Vec<Hit>, String> {
    let repo = Repo::load(services, root).map_err(|error| format!("lint failed: {error}"))?;
    let (modes, _) = modes::resolve(repo.config.erosion_gate.as_ref(), env_gate);
    let blocking = modes.blocking().into_iter();
    let checks: Vec<PairCheck> = blocking
        .filter_map(|check| PairCheck::from_name(check.name()))
        .collect();
    if checks.is_empty() {
        return Ok(Vec::new());
    }
    let revision = (services.git.closest_base(root)).unwrap_or_else(|| NO_BASE.to_owned());
    let request = LintRequest {
        form: LintForm::Revision {
            revision,
            limits: Some(LIMITS),
        },
        checks: PairSelection::of(&checks)?,
    };
    let report = worker::in_process(services, &repo, &request)
        .map_err(|error| format!("lint failed: {error}"))?;
    Ok(hits(root, report))
}

fn hits(root: &Path, report: LintReport) -> Vec<Hit> {
    let rows = report
        .candidates
        .into_iter()
        .flat_map(|candidate| candidate.findings);
    (rows.filter(|row| row.finding.severity != Severity::NotChecked))
        .map(|row| Hit {
            root: root.to_path_buf(),
            key: key_of(&row),
            finding: row.finding,
        })
        .collect()
}

// A comment finding keys on its rule and text, with no line, so a moved comment is no new finding.
fn key_of(row: &PairFinding) -> String {
    match &row.detail {
        Some(Detail::Comment(comment)) => {
            format!("{}\0{}\0{}", row.finding.check, comment.rule, comment.key)
        }
        _ => format!("{}\0{}", row.finding.check, row.finding.message),
    }
}

fn fresh(markers: &Markers, hits: Vec<Hit>) -> Vec<Hit> {
    let new = |hit: &Hit| markers.first_report(&hit.absolute(), &hit.key);
    hits.into_iter().filter(new).collect()
}

fn text(hits: &[Hit], action: &Action) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    let named: Vec<Finding> = hits.iter().map(Hit::named).collect();
    let listed: Vec<&Finding> = named.iter().collect();
    let closing = match action {
        Action::Shell(_) => EROSION_SCAN_WRITE_CLOSING,
        Action::Stop { .. } => EROSION_SCAN_STOP_CLOSING,
    };
    Some(format!("{}\n{closing}\n", verdict::blocked_text(&listed)))
}

fn entry(
    payload: Option<&Payload>,
    report: &Report,
    ts: Cow<'static, JsonValue>,
    ms: u128,
) -> Entry<'static> {
    let text = |value: Option<&str>| value.map_or(Cow::Owned(JsonValue::Null), log::text);
    let count = |count: usize| Cow::Owned(JsonValue::Int(count.to_string()));
    vec![
        (key::TS, ts),
        (HOOK_KEY, log::text(Hook::ErosionScan.name())),
        (key::EVENT, text(payload.map(|payload| payload.event))),
        (key::REASON, log::text(&report.reason)),
        (
            key::SESSION_ID,
            text(payload.map(|payload| payload.session.as_str())),
        ),
        (ROOTS_KEY, count(report.roots)),
        (FINDINGS_KEY, count(report.findings)),
        (ERROR_KEY, text(report.error.as_deref())),
        (MS_KEY, Cow::Owned(JsonValue::Int(ms.to_string()))),
    ]
}

#[cfg(test)]
mod tests;
