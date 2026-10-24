//! The `lint` steps: the flag refusals, the request, the call, the answer, and the gate.

use std::path::{Path, PathBuf};

use app::{
    After, CandidatesRequest, CheckSelection, Error, FormKind, LintCandidateInput, LintForm,
    LintReport, LintRequest, Noted, PairSelection, Services, Session, Severity,
};
use prompts::messages::{LINT_PATH_NEEDS_AFTER, LINT_SINCE_ALONE};
use render::Receipt;

use crate::Streams;
use crate::args::Arg;
use crate::exit::{EXIT_GATE, EXIT_OK, EXIT_USAGE, code_for, fail};
use crate::invocation::{Lint, Source};
use crate::steps::{answer, diagnostics, open};

// What the command line asks lint to compare, after the flag refusals.
enum Planned<'a> {
    Pairs(Compared<'a>),
    Patches(Patches<'a>),
}

// A file's proposed text, or the files changed since a revision, against the repository.
enum Compared<'a> {
    Proposed {
        path: &'a Arg,
        after: &'a Arg,
        checks: PairSelection,
    },
    Revision {
        revision: &'a Arg,
        checks: PairSelection,
    },
}

// Candidate patches: a patch file or a directory of them, or one unified diff.
enum Patches<'a> {
    Candidates(&'a Arg),
    Diff(&'a Arg),
}

pub(crate) fn run(lint: &Lint, services: &Services, streams: &mut Streams<'_>) -> u8 {
    match plan(lint) {
        Ok(Planned::Pairs(compared)) => pairs(lint, &compared, services, streams),
        Ok(Planned::Patches(source)) => patches(lint, &source, services, streams),
        Err(refusal) => fail(streams.err, &refusal, EXIT_USAGE),
    }
}

// The proposed and revision forms: the request is whole before the repository opens.
fn pairs(
    lint: &Lint,
    compared: &Compared<'_>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let request = match request(compared) {
        Ok(request) => request,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let session = match open(&lint.flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let noted = app::lint(services, &session.repo, &session.tags, &request);
    answered(lint, &session, services, noted, streams)
}

// The candidate and diff forms read their patches after the repository opens, then select the
// checks.
fn patches(
    lint: &Lint,
    source: &Patches<'_>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let session = match open(&lint.flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let loaded = match source {
        Patches::Candidates(target) => app::load_lint_candidates(services, Path::new(&target.raw)),
        Patches::Diff(target) => {
            app::load_lint_diff(services, Path::new(&target.raw)).map(|input| vec![input])
        }
    };
    let request = loaded.and_then(|candidates| candidates_request(lint, candidates));
    let request = match request {
        Ok(request) => request,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let noted = app::lint_candidates(services, &session, &request);
    answered(lint, &session, services, noted, streams)
}

fn candidates_request(
    lint: &Lint,
    candidates: Vec<LintCandidateInput>,
) -> Result<CandidatesRequest, Error> {
    let checks = lint
        .only
        .as_deref()
        .map_or(Ok(CheckSelection::all()), CheckSelection::from_names)
        .map_err(kernel::Error::OperationFailed)?;
    Ok(CandidatesRequest { candidates, checks })
}

// The warnings, then the report in the form asked for, and the gate's exit code.
fn answered(
    lint: &Lint,
    session: &Session,
    services: &Services,
    noted: Noted<Result<LintReport, Error>>,
    streams: &mut Streams<'_>,
) -> u8 {
    diagnostics(streams, &noted.diagnostics);
    let report = match noted.value {
        Ok(report) => report,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let receipt = Receipt::of(session).counted_by(&*services.counter);
    let rendered = if lint.flags.json {
        render::lint_json(&receipt, &report)
    } else {
        Ok(render::lint_text(&receipt, &report))
    };
    answer(streams, rendered, gate(&report, &lint.fail_on))
}

// The flag refusals, in order: --since with --after, --since with a patch check, --path without
// --after or --after without --path, and --path with a patch check.
fn plan(lint: &Lint) -> Result<Planned<'_>, String> {
    let only = lint.only.as_deref();
    let planned = match (&lint.source, &lint.after) {
        (Source::Since(_), Some(_)) => return Err(LINT_SINCE_ALONE.to_owned()),
        (Source::Since(revision), None) => Planned::Pairs(Compared::Revision {
            revision,
            checks: select(only, FormKind::Since)?,
        }),
        (Source::Path(path), Some(after)) => Planned::Pairs(Compared::Proposed {
            path,
            after,
            checks: select(only, FormKind::Path)?,
        }),
        (Source::Path(_), None) | (Source::Candidates(_) | Source::Diff(_), Some(_)) => {
            return Err(LINT_PATH_NEEDS_AFTER.to_owned());
        }
        (Source::Candidates(target), None) => Planned::Patches(Patches::Candidates(target)),
        (Source::Diff(target), None) => Planned::Patches(Patches::Diff(target)),
    };
    Ok(planned)
}

fn select(only: Option<&[String]>, form: FormKind) -> Result<PairSelection, String> {
    only.map_or(Ok(PairSelection::edited_files()), |names| {
        PairSelection::from_names(names, form)
    })
}

fn request(compared: &Compared<'_>) -> Result<LintRequest, String> {
    let (form, checks) = match compared {
        Compared::Proposed {
            path,
            after,
            checks,
        } => {
            let path = path.utf8("--path")?;
            let after = After::File(PathBuf::from(&after.raw));
            (LintForm::Proposed { path, after }, *checks)
        }
        Compared::Revision { revision, checks } => {
            let revision = revision.utf8("--since")?;
            (LintForm::Revision { revision }, *checks)
        }
    };
    Ok(LintRequest { form, checks })
}

// Exit 3 when a finding has a severity that --fail-on names.
fn gate(report: &LintReport, fail_on: &[Severity]) -> u8 {
    let findings = report
        .candidates
        .iter()
        .flat_map(|candidate| &candidate.findings);
    let matched = findings
        .map(|row| row.finding.severity)
        .any(|severity| fail_on.contains(&severity));
    if matched { EXIT_GATE } else { EXIT_OK }
}
