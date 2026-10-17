//! The verdicts reader: a validate JSON Lines document read back into typed values, or refused
//! with the line that is wrong.
//!
//! Every field the vote depends on must be present and of the right type, and every field that
//! names an enum value is read through that enum. Unknown fields are ignored.

use std::collections::HashMap;

use json::JsonValue;
use patch::VoteCandidate;
use text::CodePointString;

use crate::Error;
use crate::validate::{
    ApplyStatus, BaselineStatus, Member, RECORD_CANDIDATE, RECORD_KEY, RECORD_RUN, Verdict,
};

type Pairs = [(CodePointString, JsonValue)];

/// A verdicts document read back: the baseline, whether the reproduction rung is usable, the
/// commands, and one vote candidate per candidate record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedVerdicts {
    /// What the baseline established.
    pub baseline: BaselineStatus,
    /// Whether the reproduction command failed on the baseline.
    pub repro_valid: bool,
    /// The test command of the run.
    pub test_cmd: String,
    /// The reproduction command of the run.
    pub repro_cmd: Option<String>,
    /// One candidate per candidate record, in file order.
    pub candidates: Vec<VoteCandidate>,
}

/// Reads `text`, a verdicts document: a run record first, then one candidate record per line.
/// Blank lines are skipped, and a line number counts the lines that are not blank.
///
/// # Errors
///
/// Returns an operation failure that names the line for an empty document, a line that is not
/// a JSON object, a missing or mistyped field, an unknown enum value, a second run record, and a
/// repeated candidate id or index.
pub fn load_verdicts(text: &str) -> Result<LoadedVerdicts, Error> {
    read(text).map_err(|message| kernel::Error::OperationFailed(message).into())
}

fn read(text: &str) -> Result<LoadedVerdicts, String> {
    let lines = text::splitlines(text, false);
    let lines: Vec<&str> = lines
        .into_iter()
        .filter(|line| !text::strip(line).is_empty())
        .collect();
    if lines.is_empty() {
        return Err(
            "the verdicts file is empty: run `agentless-mcp validate` to produce one".to_owned(),
        );
    }
    let records = lines
        .iter()
        .enumerate()
        .map(|(position, line)| record(line, position))
        .collect::<Result<Vec<_>, String>>()?;
    let header = &records[0];
    if string(header, RECORD_KEY, 0)?.as_str() != Some(RECORD_RUN) {
        return Err(format!(
            "the first line of a verdicts file must be a '{RECORD_RUN}' record"
        ));
    }
    let candidates = candidate_records(&records)?;
    Ok(LoadedVerdicts {
        baseline: member(header, "baseline", 0)?,
        repro_valid: boolean(header, "repro_valid", 0)?,
        test_cmd: owned(string(header, "test_cmd", 0)?, "test_cmd", 0)?,
        repro_cmd: optional_string(header, "repro_cmd", 0)?,
        candidates,
    })
}

// Identity is checked here: a reader that accepts what the writer refuses lets two documents
// concatenated vote twice.
fn candidate_records(
    records: &[Vec<(CodePointString, JsonValue)>],
) -> Result<Vec<VoteCandidate>, String> {
    let mut candidates = Vec::with_capacity(records.len().saturating_sub(1));
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut indices: HashMap<i64, String> = HashMap::new();
    for (position, record) in records.iter().enumerate().skip(1) {
        let candidate = candidate_record(record, position)?;
        let line = position + 1;
        if let Some(first) = ids.get(&candidate.id) {
            return Err(format!(
                "line {line}: candidate id {} was already read at line {}. One id is one \
                 candidate, and a repeated id votes twice.",
                quoted(&candidate.id),
                first + 1
            ));
        }
        if let Some(owner) = indices.get(&candidate.index) {
            return Err(format!(
                "line {line}: candidate index {} was already read for {}. The index is the \
                 vote's tiebreak between equal-sized clusters, so two candidates cannot share one.",
                candidate.index,
                quoted(owner)
            ));
        }
        ids.insert(candidate.id.clone(), position);
        indices.insert(candidate.index, candidate.id.clone());
        candidates.push(candidate);
    }
    Ok(candidates)
}

fn candidate_record(record: &Pairs, position: usize) -> Result<VoteCandidate, String> {
    let kind = string(record, RECORD_KEY, position)?;
    let line = position + 1;
    if kind.as_str() == Some(RECORD_RUN) {
        return Err(format!(
            "line {line}: a second '{RECORD_RUN}' record. A verdicts file describes one run, so \
             two of them concatenated is not one document."
        ));
    }
    if kind.as_str() != Some(RECORD_CANDIDATE) {
        return Err(format!(
            "line {line}: '{RECORD_KEY}' is {}, which is not one of: {RECORD_RUN}, \
             {RECORD_CANDIDATE}",
            kind.quoted()
        ));
    }
    vote_candidate(record, position)
}

// The apply status and the two verdicts are read through their enums: a spelling this build
// does not know is a document it cannot rank, never a candidate that quietly falls a rung.
fn vote_candidate(record: &Pairs, position: usize) -> Result<VoteCandidate, String> {
    let Some(JsonValue::Object(apply)) = field(record, "apply") else {
        let line = position + 1;
        return Err(format!(
            "line {line}: candidate record has no 'apply' object"
        ));
    };
    let key = match field(record, "equivalence_key") {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::Str(key)) => Some(owned(key, "equivalence_key", position)?),
        Some(_) => {
            return Err(refused(
                "equivalence_key",
                position,
                "must be a string or null",
            ));
        }
    };
    let regression: Verdict = member(record, "regression", position)?;
    let reproduction: Option<Verdict> = optional_member(record, "reproduction", position)?;
    Ok(VoteCandidate {
        id: owned(string(record, "id", position)?, "id", position)?,
        index: integer(record, "index", position)?,
        applied: member::<ApplyStatus>(apply, "status", position)? == ApplyStatus::Ok,
        equivalence_key: key,
        regression: regression.regression(),
        reproduction_passed: reproduction == Some(Verdict::Passed),
    })
}

fn record(line: &str, position: usize) -> Result<Vec<(CodePointString, JsonValue)>, String> {
    let number = position + 1;
    let mut value = json::parse(line).map_err(|error| {
        format!("line {number} of the verdicts file is not valid JSON: {error}")
    })?;
    match &mut value {
        JsonValue::Object(pairs) => Ok(std::mem::take(pairs)),
        _ => Err(format!(
            "line {number} of the verdicts file is not a JSON object"
        )),
    }
}

fn field<'a>(record: &'a Pairs, name: &str) -> Option<&'a JsonValue> {
    let found = record.iter().find(|(key, _)| key.as_str() == Some(name));
    found.map(|(_, value)| value)
}

fn string<'a>(
    record: &'a Pairs,
    name: &str,
    position: usize,
) -> Result<&'a CodePointString, String> {
    match field(record, name) {
        Some(JsonValue::Str(value)) => Ok(value),
        _ => Err(missing("a string", name, position)),
    }
}

// A str holds no lone surrogate, so a text field that has one is refused here.
fn owned(value: &CodePointString, name: &str, position: usize) -> Result<String, String> {
    let text = value.as_str();
    let text = text.ok_or_else(|| refused(name, position, "has a lone surrogate"))?;
    Ok(text.to_owned())
}

fn optional_string(record: &Pairs, name: &str, position: usize) -> Result<Option<String>, String> {
    match field(record, name) {
        Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Str(value)) => owned(value, name, position).map(Some),
        _ => Err(refused(name, position, "must be a string or null")),
    }
}

fn boolean(record: &Pairs, name: &str, position: usize) -> Result<bool, String> {
    match field(record, name) {
        Some(JsonValue::Bool(value)) => Ok(*value),
        _ => Err(missing("a boolean", name, position)),
    }
}

// A bool is no integer, and an index must fit in 64 bits.
fn integer(record: &Pairs, name: &str, position: usize) -> Result<i64, String> {
    match field(record, name) {
        Some(JsonValue::Int(digits)) => digits
            .parse()
            .map_err(|_| refused(name, position, "is outside the 64-bit integer range")),
        _ => Err(missing("an integer", name, position)),
    }
}

fn member<M: Member>(record: &Pairs, name: &str, position: usize) -> Result<M, String> {
    let value = string(record, name, position)?;
    let found = M::all()
        .iter()
        .copied()
        .find(|known| value.as_str() == Some(known.name()));
    found.ok_or_else(|| {
        let allowed: Vec<&str> = M::all().iter().map(|known| known.name()).collect();
        let shown = format!(
            "is {}, which is not one of: {}",
            value.quoted(),
            allowed.join(", ")
        );
        refused(name, position, &shown)
    })
}

// Only an explicit null is no value: a missing field is refused like a mistyped one.
fn optional_member<M: Member>(
    record: &Pairs,
    name: &str,
    position: usize,
) -> Result<Option<M>, String> {
    if let Some(JsonValue::Null) = field(record, name) {
        return Ok(None);
    }
    member(record, name, position).map(Some)
}

fn missing(kind: &str, name: &str, position: usize) -> String {
    format!("line {}: missing {kind} {}", position + 1, quoted(name))
}

fn refused(name: &str, position: usize, why: &str) -> String {
    format!("line {}: {} {why}", position + 1, quoted(name))
}

fn quoted(text: &str) -> String {
    CodePointString::from(text).quoted()
}

#[cfg(test)]
mod tests;
