//! The warnings of a validate report: each case where plausible verdicts mean less than they
//! appear to, said on the error channel.

use super::records::{BaselineStatus, ReproVerdict, ValidateReport};

impl ValidateReport {
    /// Returns the warnings a caller must not miss, in a fixed order: the baseline, the
    /// reproduction command, the unmeasured candidates, the budget, and the deadline.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        [
            self.baseline_warning(),
            self.repro_warning(),
            self.unmeasured_warning(),
            self.budget_warning(),
            self.deadline_warning(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn baseline_warning(&self) -> Option<String> {
        let header = &self.header;
        if header.flaky_baseline() {
            return Some(format!(
                "UNVERIFIED: {}. No candidate was evaluated -- a baseline that answers \
                 differently on identical input cannot tell a regression your patch caused from \
                 its own noise. Find the flaky test and exclude it, or narrow the command to a \
                 subset that is deterministic.",
                header.baseline_detail
            ));
        }
        (header.baseline == BaselineStatus::Unverified).then(|| {
            format!(
                "UNVERIFIED: the test command did not pass on unpatched HEAD ({}). No candidate \
                 was evaluated -- a red baseline cannot tell a regression from a pre-existing \
                 failure.",
                header.baseline_detail
            )
        })
    }

    fn repro_warning(&self) -> Option<String> {
        match self.header.repro_verdict {
            ReproVerdict::DoesNotReproduce => Some(
                "does_not_reproduce: the reproduction command PASSED on unpatched HEAD, so it \
                 does not reproduce the bug. Its results are meaningless and it was excluded \
                 from the vote ladder. Write a test that fails before the fix."
                    .to_owned(),
            ),
            ReproVerdict::Unrunnable => {
                let run = self.header.repro_baseline_run.as_ref();
                let detail = run.map_or("no detail", |run| text::strip(&run.stderr_tail));
                Some(format!(
                    "unrunnable: the reproduction command could not be started on unpatched \
                     HEAD ({detail}). It was excluded from the vote ladder."
                ))
            }
            _ => None,
        }
    }

    fn unmeasured_warning(&self) -> Option<String> {
        let unmeasured = self.never_measured();
        (!unmeasured.is_empty()).then(|| {
            format!(
                "NOTHING WAS MEASURED for {} of {} candidates ({}): the test command could not \
                 be started in their worktrees, although it started on the baseline. Their \
                 patches applied and nothing was run against them, so they are excluded from \
                 the vote ladder rather than reported as breaking the suite.",
                unmeasured.len(),
                self.verdicts.len(),
                unmeasured.join(", ")
            )
        })
    }

    fn budget_warning(&self) -> Option<String> {
        let baseline = self.header.budget_truncated.then_some("the baseline");
        let candidates = self
            .verdicts
            .iter()
            .filter(|verdict| verdict.budget_truncated);
        let truncated: Vec<&str> = baseline
            .into_iter()
            .chain(candidates.map(|verdict| verdict.id.as_str()))
            .collect();
        (!truncated.is_empty()).then(|| {
            format!(
                "BUDGET: the run's own budget, not --timeout {}, is what bounded {}. A command \
                 cut short by the budget reports a timeout against a bound nobody chose, so read \
                 those as unfinished rather than as slow.",
                self.header.timeout,
                truncated.join(", ")
            )
        })
    }

    fn deadline_warning(&self) -> Option<String> {
        self.deadline_expired.then(|| {
            "DEADLINE: the run's overall time budget expired before evaluation completed. Work \
             it did not reach is reported not_evaluated -- raise the budget, or narrow the \
             candidate set."
                .to_owned()
        })
    }
}
