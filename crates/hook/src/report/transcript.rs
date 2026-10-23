//! The session transcripts that the log names: each call's result, applied or rejected.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use ::text::{CodePointString, PosixPath};
use json::{JsonValue, get, repr_text};

use crate::report::field::{self, Pairs};
use crate::report::log::Record;

// The transcript keys of Claude Code, not of the gate log.
const MESSAGE: &str = "message";
const CONTENT: &str = "content";
const TYPE: &str = "type";
const TOOL_RESULT: &str = "tool_result";
const TOOL_USE_ID: &str = "tool_use_id";
const IS_ERROR: &str = "is_error";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Applied,
    Rejected,
}

// Each call's result in one transcript; `results` is `None` when the file is missing.
pub(crate) struct Transcript {
    pub(crate) results: Option<HashMap<CodePointString, Status>>,
    pub(crate) unreadable_lines: usize,
}

// Each transcript the records name, read once, by its path text in sorted order.
pub(crate) type Transcripts = BTreeMap<CodePointString, Transcript>;

pub(crate) fn read_transcripts(records: &[Record]) -> Result<Transcripts, String> {
    let mut transcripts = Transcripts::new();
    for path in records
        .iter()
        .filter_map(|record| record.transcript_path.as_ref())
    {
        transcripts.entry(path.clone()).or_insert(Transcript {
            results: None,
            unreadable_lines: 0,
        });
    }
    for (path, transcript) in &mut transcripts {
        *transcript = read(path)?;
    }
    Ok(transcripts)
}

// A path with a lone surrogate ends the reference in a UnicodeEncodeError (ledger deviation).
fn read(path: &CodePointString) -> Result<Transcript, String> {
    let Some(text) = path.as_str() else {
        let shown = crate::text::lossy(path);
        return Err(format!(
            "UnicodeEncodeError: the transcript path {shown} is not UTF-8"
        ));
    };
    // The OS takes a path as a C string: a NUL in it stops the report with one ValueError line.
    if text.contains('\0') {
        return Err("ValueError: embedded null byte".to_owned());
    }
    let pure = PosixPath::parse(text).render();
    match fs::read(&pure) {
        Ok(bytes) => results(&String::from_utf8_lossy(&bytes), &pure),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Transcript {
            results: None,
            unreadable_lines: 0,
        }),
        Err(error) => {
            let shown = io::os_error_text(&error, Path::new(&pure));
            Err(format!("cannot read the transcript {pure}: {shown}"))
        }
    }
}

fn results(text: &str, pure: &str) -> Result<Transcript, String> {
    let mut results = HashMap::new();
    let mut unreadable_lines = 0;
    for (index, line) in ::text::splitlines(text, false).into_iter().enumerate() {
        if ::text::strip(line).is_empty() {
            continue;
        }
        // A live session can leave a partial last line. The report counts it.
        let Ok(document) = json::parse(line) else {
            unreadable_lines += 1;
            continue;
        };
        let at = format!("{pure}:{}", index + 1);
        for item in tool_results(&document) {
            let (tool_use_id, status) = status(item, &at)?;
            results.insert(tool_use_id, status);
        }
    }
    Ok(Transcript {
        results: Some(results),
        unreadable_lines,
    })
}

// Most transcript lines carry no tool result, so a missing key here is not an error.
fn tool_results(document: &JsonValue) -> Vec<&Pairs> {
    let content = field::seen(document)
        .dict
        .and_then(|pairs| get(pairs, MESSAGE))
        .and_then(|message| field::seen(message).dict)
        .and_then(|message| get(message, CONTENT))
        .and_then(|content| field::seen(content).list);
    let Some(items) = content else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| field::seen(item).dict)
        .filter(|item| {
            let kind = get(item, TYPE).and_then(|kind| field::seen(kind).text);
            kind.is_some_and(|kind| kind.as_str() == Some(TOOL_RESULT))
        })
        .collect()
}

// Claude Code writes no is_error key on a call that succeeded.
fn status(item: &Pairs, at: &str) -> Result<(CodePointString, Status), String> {
    let tool_use_id = field::text(item, TOOL_USE_ID, at)?;
    match get(item, IS_ERROR) {
        None | Some(JsonValue::Bool(false)) => Ok((tool_use_id, Status::Applied)),
        Some(JsonValue::Bool(true)) => Ok((tool_use_id, Status::Rejected)),
        Some(other) => {
            let shown = repr_text(other);
            Err(format!("{at}: tool_result.is_error is {shown}, not a bool"))
        }
    }
}
