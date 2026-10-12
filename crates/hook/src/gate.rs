//! The gate's steps in order. Each step either fills its record fields or halts the call.

use std::ffi::OsStr;
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Duration;

use app::{After, LintCandidate, LintForm, LintRequest, PairCheck, PairSelection, Repo, Services};

use crate::Output;
use crate::call::Call;
use crate::destination;
use crate::edit::{self, LoneSurrogate};
use crate::halt::Halt;
use crate::modes::{self, Modes};
use crate::payload::{self, Payload};
use crate::text::{panic_text, sha256_hex, shown};
use crate::verdict::{self, Decision};
use crate::worker::{self, Job, LintFn, Linted};

// A git timeout ends the call as degraded, so a hung git costs one edit at most this long.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct Ports<'a> {
    pub(crate) services: &'a Services,
    pub(crate) env_gate: Option<&'a OsStr>,
    pub(crate) lint: LintFn,
    pub(crate) binary: Option<String>,
}

struct Target {
    resolved: PathBuf,
    root: PathBuf,
    rel: String,
}

struct Configured {
    repo: Repo,
    modes: Modes,
    checks: Vec<PairCheck>,
}

// Runs the steps and settles the call. A panic in a step halts it as an internal error.
pub(crate) fn run(call: &Call, ports: &Ports<'_>, stdin: &mut dyn Read) -> Output {
    let gated = panic::catch_unwind(AssertUnwindSafe(|| gate(call, ports, stdin)));
    let decision = match gated {
        Ok(Ok(decision)) => decision,
        Ok(Err(halt)) => halted(call, halt),
        Err(payload) => {
            let message = panic_text(&*payload);
            halted(call, Halt::internal(format!("panic: {message}")))
        }
    };
    call.settle(decision)
}

fn halted(call: &Call, halt: Halt) -> Decision {
    let mut decision = Decision::default();
    if let Some(detail) = &halt.internal {
        let (id, path) = call.whose();
        decision.stderr =
            format!("erosion gate internal error: tool_use_id={id} path={path}\n{detail}\n");
    }
    call.note(|record| {
        record.outcome = Some(halt.outcome);
        record.reason = Some(halt.reason);
    });
    decision
}

fn gate(call: &Call, ports: &Ports<'_>, stdin: &mut dyn Read) -> Result<Decision, Halt> {
    let raw = payload::read(stdin)?;
    let document = payload::document(&raw)?;
    let identity = payload::identity(&document);
    call.note(|record| record.identify(identity));
    let payload = payload::payload(&document)?;
    let target = locate(call, ports, &payload)?;
    let configured = configure(call, ports, &target)?;
    let text = propose(call, &payload, &target)?;
    let candidate = lint(
        call,
        ports,
        configured.repo,
        &configured.checks,
        (target.rel, text),
    )?;
    let verdict = verdict::classify(&candidate.findings, configured.modes)?;
    let decision = verdict.decision(configured.modes)?;
    let settled = verdict.settled(candidate.observed);
    call.note(|record| record.settle(settled));
    Ok(decision)
}

fn locate(call: &Call, ports: &Ports<'_>, payload: &Payload) -> Result<Target, Halt> {
    let resolved = destination::resolve(&payload.file_path)?;
    call.note(|record| record.path = Some(shown(&resolved)));
    destination::require_tier1(&resolved)?;
    call.note(|record| record.binary.clone_from(&ports.binary));
    let budget = call
        .budget(Some(GIT_TIMEOUT))
        .ok_or_else(|| Halt::degraded("no time left for git"))?;
    let root = destination::root(&ports.services.git, &payload.file_path, budget)?;
    let rel = destination::relative(&resolved, &root)?;
    call.note(|record| {
        record.root = Some(shown(&root));
        record.rel = Some(rel.clone());
    });
    Ok(Target {
        resolved,
        root,
        rel,
    })
}

fn configure(call: &Call, ports: &Ports<'_>, target: &Target) -> Result<Configured, Halt> {
    let repo = Repo::load(ports.services, &target.root)
        .map_err(|error| Halt::degraded(format!("lint failed: {error}")))?;
    let (modes, env_warning) = modes::resolve(repo.config.erosion_gate.as_ref(), ports.env_gate);
    let mut warnings = modes::config_warnings(&repo.config);
    warnings.extend(env_warning);
    call.note(|record| {
        record.modes = Some(modes);
        record.config_warnings = warnings;
    });
    let checks: Vec<PairCheck> = modes
        .active()
        .into_iter()
        .filter_map(|check| PairCheck::from_name(check.name()))
        .collect();
    if checks.is_empty() {
        return Err(Halt::skipped("every check is off"));
    }
    Ok(Configured {
        repo,
        modes,
        checks,
    })
}

fn propose(call: &Call, payload: &Payload, target: &Target) -> Result<String, Halt> {
    let old = destination::old_text(&target.resolved)?;
    if let Some(old) = &old {
        let digest = sha256_hex(old.as_bytes());
        call.note(|record| record.old_sha256 = Some(digest));
    }
    let proposal = edit::proposal(&payload.change, old.as_deref())?;
    call.note(|record| record.eol = proposal.eol);
    proposal.text.map_err(|LoneSurrogate| {
        Halt::degraded("the proposed text is not UTF-8: surrogates not allowed")
    })
}

fn lint(
    call: &Call,
    ports: &Ports<'_>,
    repo: Repo,
    checks: &[PairCheck],
    (path, text): (String, String),
) -> Result<LintCandidate, Halt> {
    let digest = sha256_hex(text.as_bytes());
    let names = checks.iter().map(|check| check.name()).collect();
    call.note(|record| {
        record.new_sha256 = Some(digest);
        record.checks = names;
    });
    let budget = call
        .budget(None)
        .ok_or_else(|| Halt::degraded("no time left for lint"))?;
    let selection = PairSelection::of(checks).map_err(Halt::internal)?;
    let job = Job {
        services: ports.services.clone(),
        repo,
        request: LintRequest {
            form: LintForm::Proposed {
                path,
                after: After::Text(text),
            },
            checks: selection,
        },
    };
    let linted = worker::run(ports.lint, job, budget)
        .map_err(|error| Halt::internal(format!("the lint thread did not start: {error}")))?;
    candidate(linted, budget)
}

fn candidate(linted: Linted, budget: Duration) -> Result<LintCandidate, Halt> {
    let report = match linted {
        Linted::Report(report) => report,
        Linted::Failed(error) => return Err(Halt::degraded(format!("lint failed: {error}"))),
        Linted::Panicked(message) => return Err(Halt::internal(format!("panic: {message}"))),
        Linted::TimedOut => {
            let seconds = budget.as_secs_f64();
            return Err(Halt::degraded(format!(
                "lint timed out after {seconds:.1} s"
            )));
        }
    };
    let count = report.candidates.len();
    let mut candidates = report.candidates.into_iter();
    match (candidates.next(), count) {
        (Some(candidate), 1) => Ok(candidate),
        _ => Err(Halt::internal(format!(
            "lint returned {count} candidates, not one"
        ))),
    }
}

#[cfg(test)]
mod tests;
