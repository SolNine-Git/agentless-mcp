//! Lint's rows sorted into blocking and advisory findings, gaps, and checks off by design, and
//! the text and exit code each outcome gives.

use checks::{CheckGap, Detail, Finding, PairFinding, Severity};
use io::{GateMode, GatedCheck};
use serde::Serialize;

use crate::halt::Halt;
use crate::modes::Modes;
use crate::record::{Gap, Logged, Outcome, Settled};
use crate::{CONFIG_FILENAME, ENV_GATE};

const MAX_LISTED: usize = 15;
const BLOCK_EXIT: u8 = 2;
const BLOCK_HEADER: &str = "Erosion gate blocked this edit:";
const ADVISORY_HEADER: &str =
    "Erosion gate findings for this edit (advisory, the edit is not blocked):";
const ADVISORY_LABEL: &str = "Advisory findings (these do not block):";

#[derive(Debug, Default)]
pub(crate) struct Verdict<'a> {
    blocking: Vec<&'a Finding>,
    advisory: Vec<&'a Finding>,
    gaps: Vec<&'a Finding>,
    off: Vec<&'static str>,
    logged: Vec<Logged>,
}

// What the process writes: the exit code, stdout, and stderr after any diagnostic lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) code: u8,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn classify(rows: &[PairFinding], modes: Modes) -> Result<Verdict<'_>, Halt> {
    let mut verdict = Verdict::default();
    for row in rows {
        let finding = &row.finding;
        if finding.severity == Severity::NotChecked {
            if off_by_config(row) {
                verdict.off.push(finding.check);
            } else {
                verdict.gaps.push(finding);
            }
            continue;
        }
        let check = gated(finding.check).ok_or_else(|| {
            let check = finding.check;
            Halt::internal(format!("lint returned a row for {check}"))
        })?;
        let mode = modes.of(check);
        verdict.logged.push(Logged::of(row, mode));
        if mode == GateMode::Block {
            verdict.blocking.push(finding);
        } else {
            verdict.advisory.push(finding);
        }
    }
    Ok(verdict)
}

fn gated(check: &str) -> Option<GatedCheck> {
    GatedCheck::ALL
        .into_iter()
        .find(|gated| gated.name() == check)
}

// A gap that a check reports when the config or the run turns it off for this file.
fn off_by_config(row: &PairFinding) -> bool {
    let Some(Detail::Gap(gap)) = &row.detail else {
        return false;
    };
    matches!(
        (row.finding.check, gap),
        (
            checks::CONTRACT_DRIFT,
            CheckGap::NoBoundaryGlobs | CheckGap::NoRulesForLanguage(_)
        ) | (checks::CLONE_DELTA, CheckGap::NoTagCache)
    )
}

impl Verdict<'_> {
    pub(crate) fn settled(self, observed: Option<checks::Observed>) -> Settled {
        let (outcome, reason) = self.outcome();
        let gaps = self
            .gaps
            .iter()
            .map(|gap| Gap {
                check: gap.check,
                message: gap.message.clone(),
            })
            .collect();
        Settled {
            outcome,
            reason,
            gaps,
            off_by_config: self.off,
            findings: self.logged,
            observed,
        }
    }

    fn outcome(&self) -> (Outcome, Option<String>) {
        if !self.blocking.is_empty() {
            return (Outcome::Blocked, None);
        }
        if !self.advisory.is_empty() {
            return (Outcome::Advisory, None);
        }
        match self.gaps.first() {
            Some(gap) => (Outcome::Degraded, Some(gap.message.clone())),
            None => (Outcome::Clean, None),
        }
    }

    pub(crate) fn decision(&self, modes: Modes) -> Result<Decision, Halt> {
        if !self.blocking.is_empty() {
            let text = findings_text(&self.blocking, &self.advisory, modes);
            return Ok(Decision {
                code: BLOCK_EXIT,
                stderr: format!("{text}\n"),
                ..Decision::default()
            });
        }
        if self.advisory.is_empty() {
            return Ok(Decision::default());
        }
        let text = findings_text(&[], &self.advisory, modes);
        Ok(Decision {
            stdout: format!("{}\n", advisory_reply(&text)?),
            ..Decision::default()
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply<'a> {
    hook_specific_output: Context<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Context<'a> {
    hook_event_name: &'static str,
    additional_context: &'a str,
}

fn advisory_reply(text: &str) -> Result<String, Halt> {
    let reply = Reply {
        hook_specific_output: Context {
            hook_event_name: "PreToolUse",
            additional_context: text,
        },
    };
    json::to_text(&reply, json::JsonForm::Compact)
        .map_err(|error| Halt::internal(format!("TypeError: {error}")))
}

// The text the model reads: blocking findings first, at most 15 lines, then the mode.
pub(crate) fn findings_text(blocking: &[&Finding], advisory: &[&Finding], modes: Modes) -> String {
    let listed_blocking = &blocking[..blocking.len().min(MAX_LISTED)];
    let room = MAX_LISTED - listed_blocking.len();
    let listed_advisory = &advisory[..advisory.len().min(room)];
    let hidden = blocking.len() + advisory.len() - listed_blocking.len() - listed_advisory.len();
    let header = if blocking.is_empty() {
        ADVISORY_HEADER
    } else {
        BLOCK_HEADER
    };
    let mut lines = vec![header.to_owned()];
    lines.extend(listed_blocking.iter().map(|finding| line(finding)));
    if !blocking.is_empty() && !listed_advisory.is_empty() {
        lines.push(ADVISORY_LABEL.to_owned());
    }
    lines.extend(listed_advisory.iter().map(|finding| line(finding)));
    if hidden > 0 {
        lines.push(format!("...and {hidden} more findings"));
    }
    lines.push(mode_line(modes));
    lines.join("\n")
}

fn line(finding: &Finding) -> String {
    format!("{}:{}: {}", finding.path, finding.line, finding.message)
}

fn mode_line(modes: Modes) -> String {
    let current: Vec<String> = modes
        .iter()
        .map(|(check, mode)| format!("{} {}", check.name(), mode.name()))
        .collect();
    let choices: Vec<&str> = GateMode::ALL.map(GateMode::name).to_vec();
    format!(
        "Erosion gate mode: {}. Change it with the erosion_gate key in {CONFIG_FILENAME} or the \
         {ENV_GATE} variable ({}).",
        current.join(", "),
        choices.join(", ")
    )
}

#[cfg(test)]
mod tests;
