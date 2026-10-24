//! The verdicts of the candidates that were not measured: refused, not applied, or not reached.

use patch::ApplyResult;

use super::candidates::Candidate;
use super::records::{ApplyStatus, CandidateVerdict, Verdict};

// Every non-applied edit as one reason line.
pub(crate) fn reasons(result: &ApplyResult) -> Vec<String> {
    let failures = result.failures().map(|outcome| {
        format!(
            "block {} ({}) {}: {}",
            outcome.edit.index,
            outcome.edit.path,
            outcome.status.as_str(),
            outcome.reason
        )
    });
    failures.collect()
}

pub(crate) fn apply_failed(
    candidate: &Candidate,
    reasons: Vec<String>,
    duration: f64,
) -> CandidateVerdict {
    unmeasured(candidate, ApplyStatus::Failed, reasons, duration)
}

// The patch applied, so the apply status is ok; only the regression half was not evaluated.
pub(crate) fn applied_but_unmeasured(
    candidate: &Candidate,
    key: Option<String>,
    budget: &str,
    duration: f64,
) -> CandidateVerdict {
    let reason = format!("applied, then not evaluated: the run's {budget}s budget expired");
    CandidateVerdict {
        equivalence_key: key,
        budget_truncated: true,
        ..unmeasured(candidate, ApplyStatus::Ok, vec![reason], duration)
    }
}

pub(crate) fn not_evaluated(candidate: &Candidate) -> CandidateVerdict {
    let reason = "not evaluated: the baseline test run was UNVERIFIED".to_owned();
    unmeasured(candidate, ApplyStatus::NotEvaluated, vec![reason], 0.0)
}

pub(crate) fn deadline_expired(candidate: &Candidate, budget: &str) -> CandidateVerdict {
    let reason = format!("not evaluated: the run's {budget}s budget expired before this candidate");
    unmeasured(candidate, ApplyStatus::NotEvaluated, vec![reason], 0.0)
}

fn unmeasured(
    candidate: &Candidate,
    apply_status: ApplyStatus,
    apply_reasons: Vec<String>,
    duration: f64,
) -> CandidateVerdict {
    CandidateVerdict {
        id: candidate.id.clone(),
        index: candidate.index,
        apply_status,
        apply_reasons,
        equivalence_key: None,
        regression: Verdict::NotEvaluated,
        reproduction: None,
        duration,
        regression_run: None,
        reproduction_run: None,
        budget_truncated: false,
        execution_group: None,
        executed_as: None,
    }
}
