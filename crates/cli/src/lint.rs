//! The `lint` steps: the flag refusals, the request, the call, the answer, and the gate.

use std::path::PathBuf;

use app::{After, FormKind, LintForm, LintReport, LintRequest, PairSelection, Services, Severity};
use prompts::messages::{LINT_PATH_NEEDS_AFTER, LINT_SINCE_ALONE};
use render::Receipt;

use crate::Streams;
use crate::args::Arg;
use crate::exit::{EXIT_GATE, EXIT_OK, EXIT_USAGE, code_for, fail};
use crate::invocation::{Lint, Source};
use crate::steps::{answer, diagnostics, open};

// What the command line asks lint to compare, after the flag refusals.
enum Planned<'a> {
    Proposed {
        path: &'a Arg,
        after: &'a Arg,
        checks: PairSelection,
    },
    Revision {
        revision: &'a Arg,
        checks: PairSelection,
    },
    Unported(&'static str),
}

pub(crate) fn run(lint: &Lint, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let request = match plan(lint).and_then(|planned| request(&planned)) {
        Ok(request) => request,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let session = match open(&lint.flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let noted = app::lint(services, &session.repo, &session.tags, &request);
    diagnostics(streams, &noted.diagnostics);
    let report = match noted.value {
        Ok(report) => report,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let receipt = Receipt::of(&session);
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
    match (&lint.source, &lint.after) {
        (Source::Since(_), Some(_)) => Err(LINT_SINCE_ALONE.to_owned()),
        (Source::Since(revision), None) => Ok(Planned::Revision {
            revision,
            checks: select(only, FormKind::Since)?,
        }),
        (Source::Path(path), Some(after)) => Ok(Planned::Proposed {
            path,
            after,
            checks: select(only, FormKind::Path)?,
        }),
        (Source::Path(_), None) | (Source::Candidates | Source::Diff, Some(_)) => {
            Err(LINT_PATH_NEEDS_AFTER.to_owned())
        }
        (Source::Candidates, None) => Ok(Planned::Unported("--candidates")),
        (Source::Diff, None) => Ok(Planned::Unported("--diff")),
    }
}

fn select(only: Option<&[String]>, form: FormKind) -> Result<PairSelection, String> {
    only.map_or(Ok(PairSelection::edited_files()), |names| {
        PairSelection::from_names(names, form)
    })
}

fn request(planned: &Planned<'_>) -> Result<LintRequest, String> {
    let (form, checks) = match planned {
        Planned::Proposed {
            path,
            after,
            checks,
        } => {
            let path = utf8("--path", path)?;
            let after = After::File(PathBuf::from(&after.raw));
            (LintForm::Proposed { path, after }, *checks)
        }
        Planned::Revision { revision, checks } => {
            let revision = utf8("--since", revision)?;
            (LintForm::Revision { revision }, *checks)
        }
        Planned::Unported(flag) => {
            return Err(format!(
                "this build does not run lint {flag} yet. Use --path with --after, or --since, or \
                 run the Python agentless-mcp."
            ));
        }
    };
    Ok(LintRequest { form, checks })
}

fn utf8(flag: &str, arg: &Arg) -> Result<String, String> {
    let text = arg.raw.to_str().map(str::to_owned);
    text.ok_or_else(|| format!("{flag} is not valid UTF-8: {}", arg.text))
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
