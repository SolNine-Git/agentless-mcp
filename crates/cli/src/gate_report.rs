//! The gate-report command line: its parser, and the arguments it gives the erosion gate report.

use std::ffi::OsString;

use app::EnvConfig;

use crate::Streams;
use crate::args::{self, Action, Arg, Exit, Kind, Namespace, Parser, TEXT, Value};
use crate::args::{help_action, int_value};
use crate::exit::{EXIT_OK, EXIT_USAGE, fail, report, write_answer};
use crate::steps::columns;

const PROG: &str = "agentless-mcp gate-report";
const DESCRIPTION: &str = "Classify each erosion gate finding, print the trial figures per check, \
                           write a seeded labeling sample, and score a filled one against the flip \
                           criterion.";
const LOG_HELP: &str = "the JSONL file that AGENTLESS_EROSION_LOG names";
const SAMPLE_OUT_HELP: &str = "write up to 30 applied findings per check to this new CSV file";
const LABELS_HELP: &str = "score this filled CSV file (label: correct or wrong)";
const SEED_HELP: &str = "the sample seed";
const DEFAULT_SEED: &str = "0";

/// The arguments of one gate-report command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateReportArgs {
    /// The gate log.
    pub log: String,
    /// The new CSV file to write the labeling sample to.
    pub sample_out: Option<String>,
    /// The filled CSV file to score.
    pub labels: Option<String>,
    /// The seed as decimal text: a sign for a negative value, no leading zeros.
    pub seed: String,
}

/// Parses a gate-report command line, given without the program name and the command. A help
/// request or a refused line is written to `streams`, and its exit code is returned instead.
///
/// # Errors
///
/// Returns 0 after the help text, and 2 after a usage error.
pub fn gate_report_args(
    args: &[OsString],
    env: &EnvConfig,
    streams: &mut Streams<'_>,
) -> Result<GateReportArgs, u8> {
    let args: Vec<Arg> = args.iter().map(|arg| Arg::new(arg)).collect();
    match args::parse_alone(&parser(), &args, columns(env) - 2) {
        Ok(namespace) => {
            read(&namespace).map_err(|message| fail(streams.err, &message, EXIT_USAGE))
        }
        Err(Exit::Help(text)) => Err(write_answer(streams, &text, "", EXIT_OK)),
        Err(Exit::Usage(text)) => {
            report(streams.err, &text);
            Err(EXIT_USAGE)
        }
    }
}

fn parser() -> Parser {
    let log = Action {
        strings: &[],
        dest: "log",
        metavar: Some("log"),
        help: Some(LOG_HELP.to_owned()),
        kind: Kind::Positional(app::ops::Arity::One),
        required: true,
    };
    Parser {
        prog: PROG.to_owned(),
        description: Some(DESCRIPTION),
        actions: vec![
            help_action(),
            log,
            option(&["--sample-out"], "sample_out", SAMPLE_OUT_HELP),
            option(&["--labels"], "labels", LABELS_HELP),
            Action {
                kind: Kind::Value {
                    convert: int_value,
                    choices: &[],
                    repeat: false,
                },
                metavar: None,
                ..option(&["--seed"], "seed", SEED_HELP)
            },
        ],
        group: None,
    }
}

fn option(strings: &'static [&'static str], dest: &'static str, help: &str) -> Action {
    Action {
        strings,
        dest,
        metavar: Some("FILE"),
        help: Some(help.to_owned()),
        kind: Kind::Value {
            convert: TEXT,
            choices: &[],
            repeat: false,
        },
        required: false,
    }
}

// A path argument that is not UTF-8 is refused (ledger deviation).
fn read(namespace: &Namespace) -> Result<GateReportArgs, String> {
    let text = |dest: &str, name: &str| namespace.text(dest).map(|arg| arg.utf8(name)).transpose();
    let log = text("log", "log")?.unwrap_or_default();
    let seed = match namespace.get("seed") {
        Some(Value::Int(seed)) => seed.to_string(),
        _ => DEFAULT_SEED.to_owned(),
    };
    Ok(GateReportArgs {
        log,
        sample_out: text("sample_out", "--sample-out")?,
        labels: text("labels", "--labels")?,
        seed,
    })
}

#[cfg(test)]
mod tests;
