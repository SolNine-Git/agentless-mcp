//! The erosion gate trial report: each logged finding's class, the figures per check, a seeded
//! labeling sample, and the score of a filled one against the flip criterion.

mod classify;
mod csv;
mod field;
mod labels;
mod log;
mod printed;
mod random;
mod sample;
mod transcript;

#[cfg(test)]
mod tests;

use std::path::Path;

use ::text::{CodePointString, PosixPath};

use crate::Output;
use crate::report::classify::classify;
use crate::report::labels::read_labels;
use crate::report::log::read_log;
use crate::report::printed::report_text;
use crate::report::sample::{sample, sample_csv};
use crate::report::transcript::read_transcripts;

const PREFIX: &str = "erosion_gate_report: ";
const EXIT_INPUT: u8 = 1;
const EXIT_USAGE: u8 = 2;

// Why a report stops: a message for exit 1, or a sample file that already exists.
enum Stop {
    Input(String),
    Exists,
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Self::Input(message)
    }
}

/// Reports on the gate log `log`: the trial figures on stdout, the labeling sample written to the
/// new file `sample_out` with the decimal `seed`, and the labels of the filled file `labels`.
#[must_use]
pub fn gate_report(
    log: &str,
    sample_out: Option<&str>,
    labels: Option<&str>,
    seed: &str,
) -> Output {
    match run(log, sample_out, labels, seed) {
        Ok(stdout) => Output {
            stdout,
            ..Output::default()
        },
        Err(Stop::Input(message)) => Output {
            code: EXIT_INPUT,
            stderr: format!("{PREFIX}{message}\n"),
            ..Output::default()
        },
        Err(Stop::Exists) => Output {
            code: EXIT_USAGE,
            stderr: format!(
                "{PREFIX}{} exists. Name a new file, so that a filled labeling file is never \
                 overwritten.\n",
                sample_out.unwrap_or_default()
            ),
            ..Output::default()
        },
    }
}

fn run(
    log: &str,
    sample_out: Option<&str>,
    labels: Option<&str>,
    seed: &str,
) -> Result<String, Stop> {
    let records = read_log(&pure(log))?;
    let transcripts = read_transcripts(&records)?;
    let classified = classify(&records, &transcripts);
    let labels = match labels {
        Some(path) => Some(read_labels(&pure(path), &classified)?),
        None => None,
    };
    let mut notes = Vec::new();
    if let Some(out) = sample_out {
        let rows = sample(&classified, seed);
        write_sample(&sample_csv(&rows), out)?;
        let count = rows.len();
        notes.push(format!(
            "Wrote {count} findings to label to {out} (seed {seed})."
        ));
    }
    Ok(report_text(
        &records,
        &transcripts,
        &classified,
        labels.as_ref(),
        &notes,
    )?)
}

// A file the reference cannot create for a reason other than an existing one ends in its
// traceback: its last line is the message (ledger deviation).
fn write_sample(csv: &str, out: &str) -> Result<(), Stop> {
    let path = pure(out);
    match io::write_new_file(Path::new(&path), csv.as_bytes()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(Stop::Exists),
        Err(error) => {
            let quoted = CodePointString::from(path.as_str()).quoted();
            Err(Stop::Input(format!(
                "{}: {quoted}",
                io::os_error_line(&error)
            )))
        }
    }
}

// The path text that the reference opens and names: its pure POSIX form.
fn pure(path: &str) -> String {
    PosixPath::parse(path).render()
}
