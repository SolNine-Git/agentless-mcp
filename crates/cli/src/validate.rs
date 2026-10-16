//! The `validate` step: the repository, the bounds, the test command, the run, then the verdicts
//! document on stdout or in the -o file, with the receipt and the warnings on stderr.

use std::path::{Path, PathBuf};

use app::{EnvConfig, EnvRunner, IntArg, Repo, Services, ValidateReport, ValidateRequest};
use render::receipt_lines;

use crate::Streams;
use crate::caller::{below_cwd, shown_path};
use crate::exit::{EXIT_USAGE, code_for, domain_code, fail, report, write_answer};
use crate::invocation::Validate;
use crate::steps::{Resolved, diagnostics_or_stop, resolved};

// The most names that --pass-env takes.
const MAX_PASS_ENV: usize = 32;

pub(crate) fn run(
    call: &Validate,
    env: &EnvConfig,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let found = match resolved(call.repo.as_deref(), app::resolve_in_git, services, streams) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let request = match request(call, &found.repo) {
        Ok(request) => request,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    if request.test_cmd_from_repo {
        report(
            streams.err,
            &config_note(call, &found.repo, &request.test_cmd),
        );
    }
    let runner = EnvRunner::new(env);
    let noted = app::validate(services, &found.repo, &found.snapshot, &request, &runner);
    diagnostics_or_stop(streams, &noted.diagnostics);
    match noted.value {
        Ok(verdicts) => delivered(call, &found, &verdicts, services, streams),
        Err(error) => fail(streams.err, &error.to_string(), code_for(&error)),
    }
}

// The bounds, then the test command: the flag's, else the repository config's.
fn request(call: &Validate, repo: &Repo) -> Result<ValidateRequest, String> {
    bounds(call)?;
    let test_cmd = match &call.test_cmd {
        Some(given) => given.utf8("--test-cmd")?,
        None => repo.config.test_cmd.clone().ok_or_else(|| {
            format!(
                "validate needs a test command: pass --test-cmd, or set test_cmd in the \
                 repository's {}",
                io::CONFIG_FILENAME
            )
        })?,
    };
    let repro_cmd = call.repro_cmd.as_ref();
    let repro_cmd = repro_cmd.map(|given| given.utf8("--repro-cmd"));
    Ok(ValidateRequest {
        candidates: PathBuf::from(&call.candidates.raw),
        test_cmd,
        repro_cmd: repro_cmd.transpose()?,
        timeout: call.timeout.clone(),
        jobs: call.jobs.clone(),
        repeat_baseline: call.repeat_baseline.clone(),
        run_timeout: call.run_timeout.clone(),
        pass_env: once_each(&call.pass_env),
        test_cmd_from_repo: call.test_cmd.is_none(),
        allow_repo_test_cmd: call.allow_repo_test_cmd,
        allow_test_config_edits: call.allow_test_config_edits,
    })
}

// Why a number of the request is unusable, the first in flag order.
fn bounds(call: &Validate) -> Result<(), String> {
    let positive = |value: &IntArg| value.value().is_some_and(|count| count > 0);
    let refusal = if !positive(&call.timeout) {
        "--timeout takes a positive number of seconds"
    } else if !positive(&call.jobs) {
        "--jobs takes a positive integer"
    } else if !positive(&call.repeat_baseline) {
        "--repeat-baseline takes a positive integer"
    } else if call
        .run_timeout
        .as_ref()
        .is_some_and(|seconds| !positive(seconds))
    {
        "--run-timeout takes a positive number of seconds"
    } else if call.pass_env.len() > MAX_PASS_ENV {
        return Err(format!(
            "--pass-env may be repeated at most {MAX_PASS_ENV} times"
        ));
    } else {
        return Ok(());
    };
    Err(refusal.to_owned())
}

fn once_each(names: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for name in names {
        if !kept.contains(name) {
            kept.push(name.clone());
        }
    }
    kept
}

// Printed before the run: the command comes from the repository under analysis.
fn config_note(call: &Validate, repo: &Repo, test_cmd: &str) -> String {
    let opted_in = if call.allow_repo_test_cmd {
        "--allow-repo-test-cmd was given, so it will run"
    } else {
        "pass --test-cmd to name your own, or --allow-repo-test-cmd to run this one"
    };
    let path =
        (repo.config.path.as_deref()).map_or_else(String::new, |path| path.display().to_string());
    format!(
        "agentless-mcp: test command from {path}: {test_cmd}\nagentless-mcp: it comes from the \
         repository under analysis; {opted_in}.\n"
    )
}

// A document that -o cannot take goes to stdout, and the run exits 2: the verdicts are the only
// record of the run.
fn delivered(
    call: &Validate,
    found: &Resolved,
    verdicts: &ValidateReport,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let receipt = found.receipt();
    let document = render::validate_jsonl(&receipt, verdicts);
    let mut notes = String::new();
    let mut out = document;
    if let Some(output) = &call.output {
        let shown = shown_path(&output.text);
        if let Err(error) = written(Path::new(&output.raw), &out, services) {
            let message = format!("cannot write {shown}: {error}; verdicts on stdout");
            fail(streams.err, &message, EXIT_USAGE);
            return write_answer(streams, &out, "", EXIT_USAGE);
        }
        notes = format!("agentless-mcp: verdicts written to {shown}\n");
        out = String::new();
    }
    let mut lines = receipt_lines(&receipt, &verdicts.summary_line());
    let warnings = verdicts.warnings().into_iter();
    lines.extend(warnings.map(|warning| format!("agentless-mcp: {warning}")));
    notes.push_str(&(lines.join("\n") + "\n"));
    write_answer(streams, &out, &notes, domain_code(verdicts.any_passed()))
}

// The plain write of the document, with the error's text when it fails.
fn written(path: &Path, document: &str, services: &Services) -> Result<(), String> {
    let place = below_cwd(path, services);
    io::write_file(&place, document.as_bytes()).map_err(|error| io::strerror(&error))
}
