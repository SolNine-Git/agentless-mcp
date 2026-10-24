//! The records of a validate run: the four wire enums, one verdict per candidate, the run header,
//! and the report with its JSON Lines document and summary.

use std::fmt;

use io::{RunResult, RunStatus};
use json::JsonForm;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

// The key and the kinds of the record field of each verdicts line: the writer and the reader.
pub(crate) const RECORD_KEY: &str = "record";
pub(crate) const RECORD_RUN: &str = "run";
pub(crate) const RECORD_CANDIDATE: &str = "candidate";

/// Whether the unpatched repository could answer at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineStatus {
    /// The test command passed on unpatched HEAD in every baseline run.
    Ok,
    /// The baseline failed, disagreed with itself, or did not finish in the budget.
    Unverified,
}

/// What the reproduction command did on the unpatched baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReproVerdict {
    /// No reproduction command was given.
    NotGiven,
    /// It failed on the baseline: the only verdict that makes its rung usable.
    Reproduces,
    /// It passed on the baseline, so it shows nothing about a fix.
    DoesNotReproduce,
    /// It could not be started on the baseline.
    Unrunnable,
    /// The run never reached it.
    NotEvaluated,
}

/// Whether a candidate's edits landed in its worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyStatus {
    /// Every edit applied.
    Ok,
    /// An edit did not apply, or the candidate was refused before the apply.
    Failed,
    /// The run never reached the candidate.
    NotEvaluated,
}

/// One test command's outcome for one candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The command exited 0.
    Passed,
    /// The command exited with another status.
    Failed,
    /// The command outlived its bound.
    Timeout,
    /// The command could not be started.
    Error,
    /// The command never ran for this candidate.
    NotEvaluated,
}

impl BaselineStatus {
    const ALL: [Self; 2] = [Self::Ok, Self::Unverified];
    const NAMES: [&'static str; 2] = ["ok", "unverified"];

    /// Returns the value as the verdicts file spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

impl ReproVerdict {
    const ALL: [Self; 5] = [
        Self::NotGiven,
        Self::Reproduces,
        Self::DoesNotReproduce,
        Self::Unrunnable,
        Self::NotEvaluated,
    ];
    const NAMES: [&'static str; 5] = [
        "not_given",
        "reproduces",
        "does_not_reproduce",
        "unrunnable",
        "not_evaluated",
    ];

    /// Returns the value as the verdicts file spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

impl ApplyStatus {
    const ALL: [Self; 3] = [Self::Ok, Self::Failed, Self::NotEvaluated];
    const NAMES: [&'static str; 3] = ["ok", "failed", "not_evaluated"];

    /// Returns the value as the verdicts file spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

impl Verdict {
    const ALL: [Self; 5] = [
        Self::Passed,
        Self::Failed,
        Self::Timeout,
        Self::Error,
        Self::NotEvaluated,
    ];
    const NAMES: [&'static str; 5] = ["passed", "failed", "timeout", "error", "not_evaluated"];

    /// Returns the value as the verdicts file spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// Returns true when the command ran and this verdict is what it gave.
    #[must_use]
    pub fn measured(self) -> bool {
        !matches!(self, Self::Error | Self::NotEvaluated)
    }

    pub(crate) fn of(status: RunStatus) -> Verdict {
        match status {
            RunStatus::Passed => Self::Passed,
            RunStatus::Failed => Self::Failed,
            RunStatus::Timeout => Self::Timeout,
            RunStatus::Error => Self::Error,
        }
    }

    // The ladder reads a regression verdict as passed, failed, or not measured at all.
    pub(crate) fn regression(self) -> patch::Regression {
        match self {
            Self::Passed => patch::Regression::Passed,
            Self::Failed | Self::Timeout => patch::Regression::Failed,
            Self::Error | Self::NotEvaluated => patch::Regression::NotMeasured,
        }
    }
}

// The values of one wire enum, which the verdicts reader reads a field through.
pub(crate) trait Member: Copy + 'static {
    fn all() -> &'static [Self];
    fn name(self) -> &'static str;
}

macro_rules! wire_values {
    ($($kind:ident),*) => {$(
        impl Member for $kind {
            fn all() -> &'static [Self] {
                &Self::ALL
            }

            fn name(self) -> &'static str {
                self.as_str()
            }
        }

        impl fmt::Display for $kind {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $kind {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
    )*};
}

wire_values!(BaselineStatus, ReproVerdict, ApplyStatus, Verdict);

/// Everything one candidate produced, including why it produced nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateVerdict {
    /// The file stem of the candidate.
    pub id: String,
    /// The place of the candidate in the sorted directory.
    pub index: usize,
    /// Whether its edits landed.
    pub apply_status: ApplyStatus,
    /// Why its edits did not land, or why it was not evaluated.
    pub apply_reasons: Vec<String>,
    /// The equivalence key of its change, for a candidate that applied and changed a file.
    pub equivalence_key: Option<String>,
    /// The regression command's verdict.
    pub regression: Verdict,
    /// The reproduction command's verdict, when the reproduction rung is usable.
    pub reproduction: Option<Verdict>,
    /// The seconds the candidate took, to 3 decimal places; 0 for a shared execution.
    pub duration: f64,
    /// The regression run, kept for the tails of a run that did not pass.
    pub regression_run: Option<RunResult>,
    /// The reproduction run, kept for the tails of a run that did not pass.
    pub reproduction_run: Option<RunResult>,
    /// True when the run's budget, not the per-command timeout, bounded this candidate.
    pub budget_truncated: bool,
    /// The fingerprint of the changed files that this candidate's group shares.
    pub execution_group: Option<String>,
    /// The id of the candidate whose execution this record carries.
    pub executed_as: Option<String>,
}

impl CandidateVerdict {
    /// Returns true when the candidate applied and has a non-empty equivalence key, so it can
    /// vote.
    #[must_use]
    pub fn usable(&self) -> bool {
        let keyed = self
            .equivalence_key
            .as_deref()
            .is_some_and(|key| !key.is_empty());
        self.apply_status == ApplyStatus::Ok && keyed
    }

    fn record(&self) -> CandidateRecord<'_> {
        CandidateRecord {
            record: RECORD_CANDIDATE,
            id: &self.id,
            index: self.index,
            apply: ApplyRecord {
                status: self.apply_status,
                reasons: &self.apply_reasons,
            },
            regression: self.regression,
            reproduction: self.reproduction,
            equivalence_key: self.equivalence_key.as_deref(),
            duration: self.duration,
            budget_truncated: self.budget_truncated,
            tails: Tails {
                regression: failed(self.regression_run.as_ref()),
                reproduction: failed(self.reproduction_run.as_ref()),
            },
            execution_group: self.execution_group.as_deref(),
            executed_as: self.executed_as.as_deref(),
        }
    }
}

/// The run record: what was asked and what the baseline said. The receipt is not part of it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunHeader {
    /// The test command.
    pub test_cmd: String,
    /// The reproduction command.
    pub repro_cmd: Option<String>,
    /// The bound of one command, in seconds.
    pub timeout: usize,
    /// The representatives that may run at once.
    pub jobs: usize,
    /// The number of candidates.
    pub candidates: usize,
    /// What the baseline established.
    pub baseline: BaselineStatus,
    /// The baseline in one line.
    pub baseline_detail: String,
    /// What the reproduction command did on the baseline.
    pub repro_verdict: ReproVerdict,
    /// The baseline run that the detail describes.
    pub baseline_run: Option<RunResult>,
    /// The reproduction command's baseline run.
    pub repro_baseline_run: Option<RunResult>,
    /// The baseline runs that were asked for.
    pub repeat_baseline: usize,
    /// The baseline runs that did not pass.
    pub baseline_failures: usize,
    /// Whether a candidate could edit the files that name what the tests run.
    pub allow_test_config_edits: bool,
    /// True when the run's budget, not the per-command timeout, bounded a baseline command.
    pub budget_truncated: bool,
}

impl RunHeader {
    /// Returns true only when the reproduction command failed on unpatched HEAD.
    #[must_use]
    pub fn repro_valid(&self) -> bool {
        self.repro_verdict == ReproVerdict::Reproduces
    }

    /// Returns true when the baseline runs disagreed with each other.
    #[must_use]
    pub fn flaky_baseline(&self) -> bool {
        0 < self.baseline_failures && self.baseline_failures < self.repeat_baseline
    }
}

/// One validate run: the header, a verdict per candidate in index order, and whether the run's
/// budget stopped work the run would otherwise have done.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidateReport {
    /// The run record.
    pub header: RunHeader,
    /// One verdict per candidate, in index order.
    pub verdicts: Vec<CandidateVerdict>,
    /// True when the budget expired before evaluation completed.
    pub deadline_expired: bool,
}

impl ValidateReport {
    /// Returns true when a candidate applied, has an equivalence key and passed the regression
    /// command: a decisive tier of the vote.
    #[must_use]
    pub fn any_passed(&self) -> bool {
        self.verdicts
            .iter()
            .any(|verdict| verdict.usable() && verdict.regression == Verdict::Passed)
    }

    /// Returns the JSON Lines document: the run record with `receipt` as its receipt, then one
    /// record per candidate, each on one line.
    ///
    /// # Panics
    ///
    /// Panics when `receipt` does not serialize to JSON, for example with a map key that is not
    /// a string.
    #[must_use]
    pub fn jsonl<R: Serialize + ?Sized>(&self, receipt: &R) -> String {
        let header = RunRecord {
            header: &self.header,
            receipt,
        };
        let mut document = line(&header);
        for verdict in &self.verdicts {
            document.push_str(&line(&verdict.record()));
        }
        document
    }

    /// Returns the one-line summary for the receipt.
    #[must_use]
    pub fn summary_line(&self) -> String {
        let passed = self.count(|verdict| verdict.regression == Verdict::Passed);
        let applied = self.count(|verdict| verdict.apply_status == ApplyStatus::Ok);
        format!(
            "baseline {}; {applied} of {} candidates applied, {passed} passed the regression suite",
            self.header.baseline,
            self.verdicts.len()
        )
    }

    // The ids of the candidates that applied and whose test command never started. A candidate
    // that the budget stopped after its apply is not here: the budget warning names it.
    pub(crate) fn never_measured(&self) -> Vec<&str> {
        let unmeasured = self.verdicts.iter().filter(|verdict| {
            verdict.apply_status == ApplyStatus::Ok
                && !verdict.regression.measured()
                && !verdict.budget_truncated
        });
        unmeasured.map(|verdict| verdict.id.as_str()).collect()
    }

    fn count(&self, wanted: impl Fn(&CandidateVerdict) -> bool) -> usize {
        self.verdicts
            .iter()
            .filter(|verdict| wanted(verdict))
            .count()
    }
}

// A green run carries no tail: it would bury the records that matter.
fn failed(run: Option<&RunResult>) -> Option<&RunResult> {
    run.filter(|run| !run.passed())
}

fn line(record: &impl Serialize) -> String {
    let mut text = json::to_text(record, JsonForm::Compact).expect("a verdicts record serializes");
    text.push('\n');
    text
}

// The run record: the receipt second, then the header's fields and the two derived flags.
struct RunRecord<'a, R: ?Sized> {
    header: &'a RunHeader,
    receipt: &'a R,
}

impl<R: Serialize + ?Sized> Serialize for RunRecord<'_, R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let header = self.header;
        let mut form = serializer.serialize_struct("RunRecord", 18)?;
        form.serialize_field(RECORD_KEY, RECORD_RUN)?;
        form.serialize_field("receipt", self.receipt)?;
        form.serialize_field("test_cmd", &header.test_cmd)?;
        form.serialize_field("repro_cmd", &header.repro_cmd)?;
        form.serialize_field("timeout", &header.timeout)?;
        form.serialize_field("jobs", &header.jobs)?;
        form.serialize_field("candidates", &header.candidates)?;
        form.serialize_field("baseline", &header.baseline)?;
        form.serialize_field("baseline_detail", &header.baseline_detail)?;
        form.serialize_field("repeat_baseline", &header.repeat_baseline)?;
        form.serialize_field("baseline_failures", &header.baseline_failures)?;
        form.serialize_field("flaky_baseline", &header.flaky_baseline())?;
        form.serialize_field("budget_truncated", &header.budget_truncated)?;
        form.serialize_field("repro_verdict", &header.repro_verdict)?;
        form.serialize_field("repro_valid", &header.repro_valid())?;
        form.serialize_field("allow_test_config_edits", &header.allow_test_config_edits)?;
        form.serialize_field("baseline_run", &header.baseline_run)?;
        form.serialize_field("repro_baseline_run", &header.repro_baseline_run)?;
        form.end()
    }
}

#[derive(Serialize)]
struct CandidateRecord<'a> {
    record: &'static str,
    id: &'a str,
    index: usize,
    apply: ApplyRecord<'a>,
    regression: Verdict,
    reproduction: Option<Verdict>,
    equivalence_key: Option<&'a str>,
    duration: f64,
    budget_truncated: bool,
    #[serde(skip_serializing_if = "Tails::is_empty")]
    tails: Tails<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_group: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executed_as: Option<&'a str>,
}

#[derive(Serialize)]
struct ApplyRecord<'a> {
    status: ApplyStatus,
    reasons: &'a [String],
}

// The output tails of the runs that did not pass.
#[derive(Serialize)]
struct Tails<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    regression: Option<&'a RunResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reproduction: Option<&'a RunResult>,
}

impl Tails<'_> {
    fn is_empty(&self) -> bool {
        self.regression.is_none() && self.reproduction.is_none()
    }
}
