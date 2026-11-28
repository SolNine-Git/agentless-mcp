//! `linter_delta`: each linter finding that a change adds, keyed by its code and its source line,
//! so a finding that the old text already had is no finding.

use std::collections::HashMap;
use std::time::Duration;

use prompts::{Linter, messages};
use serde::Deserialize;

use crate::error::Error;
use crate::finding::{CheckGap, Detail, Finding, LinterKey, PairFinding, Severity};
use crate::finding::{not_checked, sorted};
use crate::observed::{LinterState, Observed, Outcome};
use crate::pair::{Pair, refuse_repeats};

/// One finding of a linter run, as the linter's JSON output gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinterFinding {
    /// The rule code, such as `F401`.
    pub code: String,
    /// The linter's message.
    pub message: String,
    /// The 1-based line that the finding starts on.
    pub row: u32,
    /// The message of the linter's fix, when it gives one.
    pub fix: Option<String>,
}

/// Why `linter_delta` did not lint a pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotLinted {
    /// No config file names the linter's settings from the file's folder up to the root.
    NoConfig,
    /// The linter's program is neither in the repository nor on `PATH`.
    NoBinary,
    /// A run of the linter failed.
    Failed(LinterFailure),
}

/// How one run of the linter failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinterFailure {
    /// The run ended with a status other than 0, or a signal ended it.
    Exited {
        /// The exit status as the operating system shows it.
        status: String,
        /// The first line of stderr, or `None` when stderr was empty.
        stderr: Option<String>,
    },
    /// The run passed its timeout.
    TimedOut(Duration),
    /// The call had no time left for the run, so it did not start.
    BudgetSpent,
    /// The output is not the JSON list of findings that the linter prints.
    Unparsed(String),
    /// The output passed the cap, in bytes, so the run was killed.
    Capped(usize),
    /// The run did not start, or its result was lost after the start.
    NotRun(String),
}

/// The findings of a pair's two texts. A text that the change adds has no old findings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintedSides {
    /// The findings of the text before the change.
    pub old: Vec<LinterFinding>,
    /// The findings of the text after the change.
    pub new: Vec<LinterFinding>,
}

/// One pair with a new text, its linter, and the findings of both texts or why it has none.
pub struct LintedPair<'a> {
    /// The pair.
    pub pair: &'a Pair,
    /// The linter of the pair's language.
    pub linter: &'static Linter,
    /// The findings, or why the pair was not linted.
    pub linted: Result<LintedSides, NotLinted>,
}

// ruff's JSON: a list of findings. Every other field of a finding is left unread.
#[derive(Deserialize)]
struct RuffFinding {
    code: String,
    message: String,
    location: RuffLocation,
    fix: Option<RuffFix>,
}

#[derive(Deserialize)]
struct RuffLocation {
    row: u32,
}

#[derive(Deserialize)]
struct RuffFix {
    message: Option<String>,
}

/// Returns the findings of one run, read from stdout only on exit 0, or why the run gave none.
///
/// # Errors
///
/// Returns [`NotLinted::NoBinary`] when the program was not found, and [`NotLinted::Failed`]
/// for every other run that did not exit 0 with stdout that parses.
pub fn ran(
    outcome: io::Outcome,
    timeout: Duration,
    stdout_cap: usize,
) -> Result<Vec<LinterFinding>, NotLinted> {
    let failure = match outcome {
        io::Outcome::Exited { status, stdout, .. } if status.success() => {
            return ruff_findings(&stdout).map_err(|error| failed(LinterFailure::Unparsed(error)));
        }
        io::Outcome::Exited { status, stderr, .. } => LinterFailure::Exited {
            status: status.to_string(),
            stderr: first_line(&stderr),
        },
        io::Outcome::Truncated { .. } => LinterFailure::Capped(stdout_cap),
        io::Outcome::TimedOut => LinterFailure::TimedOut(timeout),
        io::Outcome::NotRun(io::NotRun::ProgramNotFound(_)) => return Err(NotLinted::NoBinary),
        io::Outcome::NotRun(not_run) => LinterFailure::NotRun(not_run.to_string()),
        io::Outcome::Lost(error) => LinterFailure::NotRun(io::strerror(&error)),
    };
    Err(failed(failure))
}

fn failed(failure: LinterFailure) -> NotLinted {
    NotLinted::Failed(failure)
}

fn ruff_findings(stdout: &[u8]) -> Result<Vec<LinterFinding>, String> {
    let parsed: Vec<RuffFinding> =
        serde_json::from_slice(stdout).map_err(|error| error.to_string())?;
    Ok(parsed
        .into_iter()
        .map(|finding| LinterFinding {
            code: finding.code,
            message: finding.message,
            row: finding.location.row,
            fix: finding
                .fix
                .and_then(|fix| fix.message)
                .filter(|message| !message.is_empty()),
        })
        .collect())
}

fn first_line(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

/// Reports each linter finding a change adds, and a `not_checked` row for each pair not linted.
///
/// # Errors
///
/// Returns an error when two pairs name one path.
pub fn linter_delta(pairs: &[LintedPair<'_>]) -> Result<Outcome, Error> {
    let measured: Vec<&Pair> = pairs.iter().map(|linted| linted.pair).collect();
    refuse_repeats(&measured)?;
    let mut findings = Vec::new();
    let mut states: Option<Vec<LinterState>> = None;
    for linted in pairs {
        let (Some(new_text), path) = (linted.pair.new_text(), linted.pair.path()) else {
            continue;
        };
        let sides = match &linted.linted {
            Ok(sides) => sides,
            Err(not_linted) => {
                findings.push(gap_row(path, linted.linter, not_linted));
                continue;
            }
        };
        let old = match linted.pair.old_text() {
            Some(old_text) => keyed(linted.linter, old_text, &sides.old),
            None => Vec::new(),
        };
        let new = keyed(linted.linter, new_text, &sides.new);
        let found = states.get_or_insert_with(Vec::new);
        found.extend(new.iter().map(|keyed| LinterState {
            path: path.to_owned(),
            code: keyed.finding.code.clone(),
            key: keyed.key.clone(),
        }));
        let added = fresh(&old, new);
        findings.extend(
            added
                .into_iter()
                .map(|keyed| row(path, linted.linter, keyed)),
        );
    }
    if let Some(states) = &mut states {
        states.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(Outcome {
        findings: sorted(findings),
        observed: Observed {
            linter_keys: states,
            ..Observed::default()
        },
    })
}

// A finding and its key: the source line with whitespace collapsed, or empty for a syntax error.
struct Keyed<'a> {
    finding: &'a LinterFinding,
    key: String,
}

// Every syntax-error finding of one text makes one key, at the first of them.
fn keyed<'a>(linter: &Linter, text: &str, findings: &'a [LinterFinding]) -> Vec<Keyed<'a>> {
    let lines = lines(text);
    let mut syntax_seen = false;
    let mut keyed = Vec::new();
    for finding in findings {
        if finding.code == linter.syntax_code {
            if !syntax_seen {
                keyed.push(Keyed {
                    finding,
                    key: String::new(),
                });
            }
            syntax_seen = true;
            continue;
        }
        let row = usize::try_from(finding.row).unwrap_or(usize::MAX);
        let line = row.checked_sub(1).and_then(|index| lines.get(index));
        let key = text::split_whitespace(line.copied().unwrap_or("")).collect::<Vec<_>>();
        keyed.push(Keyed {
            finding,
            key: key.join(" "),
        });
    }
    keyed
}

// The lines as a linter counts rows: a break is `\r\n`, `\n` or `\r`.
fn lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['\n', '\r']) {
        lines.push(&rest[..at]);
        let width = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[at + width..];
    }
    lines.push(rest);
    lines
}

impl Keyed<'_> {
    fn id(&self) -> (String, String) {
        (self.finding.code.clone(), self.key.clone())
    }
}

// The new findings whose key the old text does not hold as many times, in the new text's order.
fn fresh<'a>(old: &[Keyed<'_>], new: Vec<Keyed<'a>>) -> Vec<Keyed<'a>> {
    let mut carried: HashMap<(String, String), usize> = HashMap::new();
    for keyed in old {
        *carried.entry(keyed.id()).or_default() += 1;
    }
    let mut carried_away = |keyed: &Keyed<'_>| match carried.get_mut(&keyed.id()) {
        Some(left) if *left > 0 => {
            *left -= 1;
            true
        }
        _ => false,
    };
    new.into_iter()
        .filter(|keyed| !carried_away(keyed))
        .collect()
}

fn row(path: &str, linter: &Linter, keyed: Keyed<'_>) -> PairFinding {
    let found = keyed.finding;
    let (code, name, message) = (found.code.as_str(), linter.name, found.message.as_str());
    let message = match &found.fix {
        Some(fix) => messages::LINTER_FINDING_FIX.render((code, name, fix, message)),
        None => messages::LINTER_FINDING.render((code, name, message)),
    };
    let finding = Finding {
        check: crate::LINTER_DELTA,
        severity: Severity::Advisory,
        message,
        path: path.to_owned(),
        line: found.row,
        evidence: messages::LINTER_EVIDENCE.render(code),
    };
    let key = LinterKey {
        code: found.code.clone(),
        key: keyed.key,
    };
    PairFinding {
        finding,
        detail: Some(Detail::Linter(key)),
    }
}

fn gap_row(path: &str, linter: &Linter, not_linted: &NotLinted) -> PairFinding {
    let name = linter.name;
    let (gap, message) = match not_linted {
        NotLinted::NoConfig => {
            let (configs, table) = (linter.configs.join(", "), linter.table.join("."));
            let values = (
                linter.manifest,
                path,
                name,
                configs.as_str(),
                table.as_str(),
            );
            (
                CheckGap::NoLinterConfig,
                messages::LINTER_NO_CONFIG.render(values),
            )
        }
        NotLinted::NoBinary => (
            CheckGap::NoLinterBinary,
            messages::LINTER_NO_BINARY.render((linter.local, path, name)),
        ),
        NotLinted::Failed(failure) => {
            let reason = reason(failure);
            let message = messages::LINTER_FAILED.render((path, name, reason.as_str()));
            (CheckGap::LinterFailed, message)
        }
    };
    not_checked(crate::LINTER_DELTA, path, message, gap.reason(), Some(gap))
}

fn reason(failure: &LinterFailure) -> String {
    match failure {
        LinterFailure::Exited {
            status,
            stderr: Some(stderr),
        } => messages::LINTER_EXITED.render((status, stderr)),
        LinterFailure::Exited { status, .. } => messages::LINTER_EXITED_SILENT.render(status),
        LinterFailure::TimedOut(timeout) => {
            let seconds = format!("{:.1}", timeout.as_secs_f64());
            messages::LINTER_TIMED_OUT.render(seconds.as_str())
        }
        LinterFailure::BudgetSpent => messages::LINTER_BUDGET_SPENT.to_owned(),
        LinterFailure::Unparsed(error) => messages::LINTER_UNPARSED.render(error),
        LinterFailure::Capped(cap) => messages::LINTER_OUTPUT_CAPPED.render(*cap),
        LinterFailure::NotRun(reason) => messages::LINTER_NOT_RUN.render(reason),
    }
}

#[cfg(test)]
mod tests;
