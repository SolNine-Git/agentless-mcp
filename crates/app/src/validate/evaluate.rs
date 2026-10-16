//! The evaluation phase: each representative applied in place in its own worktree, the
//! regression command run there, then the reproduction command; more than one at once when
//! the caller asks. The answers come back in representative order, whatever the completion.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use git::Worktree;
use io::RunResult;

use super::budget;
use super::built::{applied_but_unmeasured, apply_failed, deadline_expired, reasons};
use super::candidates::Candidate;
use super::plan::{Plan, execution_groups, share_execution};
use super::records::{ApplyStatus, CandidateVerdict, Verdict};
use super::{Run, and_then, is_stopped, map_ok, stopped, worktree_repo};
use crate::parallel;
use crate::patches::{ApplyMode, apply_patch};
use crate::{Error, Noted};

// One representative's verdict, and whether the budget abandoned work it would have had.
#[derive(Clone, Debug, PartialEq)]
struct Evaluation {
    verdict: CandidateVerdict,
    abandoned: bool,
}

// Why a representative has no evaluation: another one failed first, or this one failed.
enum Halt {
    Skipped,
    Failed(Error),
}

// What the reproduction rung gave one candidate.
struct Reproduced {
    verdict: Option<Verdict>,
    run: Option<RunResult>,
    truncated: bool,
    abandoned: bool,
}

impl Reproduced {
    fn not_given() -> Reproduced {
        Reproduced {
            verdict: None,
            run: None,
            truncated: false,
            abandoned: false,
        }
    }

    fn not_evaluated(abandoned: bool) -> Reproduced {
        Reproduced {
            verdict: Some(Verdict::NotEvaluated),
            abandoned,
            ..Reproduced::not_given()
        }
    }
}

impl Run<'_> {
    // Every verdict in index order, and whether the budget abandoned work.
    pub(super) fn evaluate_all(
        &self,
        candidates: &[Candidate],
        repro_valid: bool,
    ) -> Noted<Result<(Vec<CandidateVerdict>, bool), Error>> {
        and_then(self.plan_candidates(candidates), |(plans, immediate)| {
            self.executed(&plans, immediate, repro_valid)
        })
    }

    fn executed(
        &self,
        plans: &[Plan<'_>],
        immediate: Vec<CandidateVerdict>,
        repro_valid: bool,
    ) -> Noted<Result<(Vec<CandidateVerdict>, bool), Error>> {
        let groups = execution_groups(plans);
        let representatives: Vec<&Plan<'_>> = groups.iter().map(|group| &plans[group[0]]).collect();
        let jobs = self.limits.jobs;
        let threads = if jobs > 1 && representatives.len() > 1 {
            jobs
        } else {
            1
        };
        let evaluated = parallel::map_in_order(&representatives, threads, |plan| {
            self.evaluate(plan, repro_valid)
        });
        map_ok(gathered(evaluated), |evaluations| {
            shared(plans, &groups, &evaluations, immediate)
        })
    }

    fn evaluate(&self, plan: &Plan<'_>, repro_valid: bool) -> Noted<Result<Evaluation, Halt>> {
        if self.stop.load(Ordering::SeqCst) {
            return Noted::new(Err(Halt::Skipped));
        }
        if self.runner.interrupted() {
            self.stop.store(true, Ordering::SeqCst);
            return Noted::new(Err(Halt::Failed(stopped())));
        }
        if self.reached() {
            let verdict = deadline_expired(plan.candidate, &self.budget());
            return Noted::new(Ok(Evaluation {
                verdict,
                abandoned: true,
            }));
        }
        let started = self.now();
        let judged = self.in_worktree(|tree| self.judged(tree, plan, started, repro_valid));
        if judged.value.is_err() {
            self.stop.store(true, Ordering::SeqCst);
        }
        Noted {
            value: judged.value.map_err(Halt::Failed),
            diagnostics: judged.diagnostics,
        }
    }

    // The worktree is the repository the candidate is judged in, so every path of the patch is
    // contained against it, and a path outside it refuses the whole run.
    fn judged(
        &self,
        tree: &Worktree,
        plan: &Plan<'_>,
        started: Instant,
        repro_valid: bool,
    ) -> Noted<Result<Evaluation, Error>> {
        let repo = match worktree_repo(tree) {
            Ok(repo) => repo,
            Err(error) => return Noted::new(Err(error)),
        };
        let git = &self.services.git;
        let snapshot = git.snapshot(&repo.root, git.root(&repo.root).as_deref());
        let applied = apply_patch(
            self.services,
            &repo,
            &snapshot,
            &plan.edits,
            ApplyMode::InPlace,
        );
        and_then(applied, |report| {
            if !report.ok() {
                let reasons = reasons(&report.result);
                let verdict = apply_failed(plan.candidate, reasons, self.elapsed(started));
                return Noted::new(Ok(Evaluation {
                    verdict,
                    abandoned: false,
                }));
            }
            self.measured(tree.inside(), plan, started, repro_valid)
        })
    }

    fn measured(
        &self,
        cwd: &Path,
        plan: &Plan<'_>,
        started: Instant,
        repro_valid: bool,
    ) -> Noted<Result<Evaluation, Error>> {
        let Some(bound) = self.bound() else {
            let key = plan.equivalence_key.clone();
            let elapsed = self.elapsed(started);
            let verdict = applied_but_unmeasured(plan.candidate, key, &self.budget(), elapsed);
            return Noted::new(Ok(Evaluation {
                verdict,
                abandoned: true,
            }));
        };
        let regression = self.run_command(cwd, &self.request.test_cmd, &bound);
        and_then(regression, |regression| {
            let truncated = budget::truncated(&bound, &regression);
            let reproduced = self.reproduction(cwd, &regression, repro_valid);
            map_ok(reproduced, |reproduced| {
                let duration = self.elapsed(started);
                measured_verdict(plan, regression, reproduced, truncated, duration)
            })
        })
    }

    // The reproduction command runs only on a usable rung and after a passing regression run.
    fn reproduction(
        &self,
        cwd: &Path,
        regression: &RunResult,
        repro_valid: bool,
    ) -> Noted<Result<Reproduced, Error>> {
        let command = match self.request.repro_cmd.as_deref() {
            Some(command) if repro_valid => command,
            _ => return Noted::new(Ok(Reproduced::not_given())),
        };
        if !regression.passed() {
            return Noted::new(Ok(Reproduced::not_evaluated(false)));
        }
        let Some(bound) = self.bound() else {
            return Noted::new(Ok(Reproduced::not_evaluated(true)));
        };
        map_ok(self.run_command(cwd, command, &bound), |run| Reproduced {
            verdict: Some(Verdict::of(run.status)),
            truncated: budget::truncated(&bound, &run),
            run: Some(run),
            abandoned: false,
        })
    }
}

fn measured_verdict(
    plan: &Plan<'_>,
    regression: RunResult,
    reproduced: Reproduced,
    truncated: bool,
    duration: f64,
) -> Evaluation {
    let verdict = CandidateVerdict {
        id: plan.candidate.id.clone(),
        index: plan.candidate.index,
        apply_status: ApplyStatus::Ok,
        apply_reasons: Vec::new(),
        equivalence_key: plan.equivalence_key.clone(),
        regression: Verdict::of(regression.status),
        reproduction: reproduced.verdict,
        duration,
        regression_run: Some(regression),
        reproduction_run: reproduced.run,
        budget_truncated: truncated || reproduced.truncated,
        execution_group: None,
        executed_as: None,
    };
    Evaluation {
        verdict,
        abandoned: reproduced.abandoned,
    }
}

// The diagnostics of every representative in order. A SIGINT wins over any other failure,
// and otherwise the first failure in representative order is the answer.
fn gathered(
    evaluated: Vec<Noted<Result<Evaluation, Halt>>>,
) -> Noted<Result<Vec<Evaluation>, Error>> {
    let mut diagnostics = Vec::new();
    let mut evaluations = Vec::with_capacity(evaluated.len());
    let mut failure: Option<Error> = None;
    let mut skipped = false;
    for noted in evaluated {
        diagnostics.extend(noted.diagnostics);
        match noted.value {
            Ok(evaluation) => evaluations.push(evaluation),
            Err(Halt::Skipped) => skipped = true,
            Err(Halt::Failed(error)) => failure = Some(kept(failure, error)),
        }
    }
    let value = match failure {
        Some(error) => Err(error),
        // A representative is skipped only after another failed, so this is a stopped call.
        None if skipped => Err(stopped()),
        None => Ok(evaluations),
    };
    Noted { value, diagnostics }
}

fn kept(first: Option<Error>, found: Error) -> Error {
    match first {
        Some(first) if is_stopped(&first) || !is_stopped(&found) => first,
        _ => found,
    }
}

// Each representative's execution projected onto every member of its group, with the
// candidates that never planned, in index order.
fn shared(
    plans: &[Plan<'_>],
    groups: &[Vec<usize>],
    evaluations: &[Evaluation],
    immediate: Vec<CandidateVerdict>,
) -> (Vec<CandidateVerdict>, bool) {
    let mut verdicts = immediate;
    for (group, evaluation) in groups.iter().zip(evaluations) {
        let representative = &plans[group[0]].candidate.id;
        let members = group.iter().map(|&member| &plans[member]);
        verdicts
            .extend(members.map(|plan| share_execution(&evaluation.verdict, plan, representative)));
    }
    verdicts.sort_by_key(|verdict| verdict.index);
    let abandoned = evaluations.iter().any(|evaluation| evaluation.abandoned);
    (verdicts, abandoned)
}
