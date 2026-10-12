//! The `PreToolUse` payload: the bounded read, the JSON parse, and the typed Write or Edit call.

use std::io::Read;
use std::path::{Path, PathBuf};

use json::{JsonErrorKind, JsonValue};
use text::CodePointString;

use crate::halt::Halt;
use crate::record::Identity;
use crate::text::{codec_text, lossy, os_error_line};

const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const WRITE: &str = "Write";
const EDIT: &str = "Edit";

type Fields = [(CodePointString, JsonValue)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditInput {
    pub(crate) old_string: CodePointString,
    pub(crate) new_string: CodePointString,
    pub(crate) replace_all: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Write(CodePointString),
    Edit(EditInput),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Payload {
    // An absolute path, or a relative one joined to the payload's `cwd`.
    pub(crate) file_path: PathBuf,
    pub(crate) change: Change,
}

pub(crate) fn read(stdin: &mut dyn Read) -> Result<Vec<u8>, Halt> {
    let mut raw = Vec::new();
    let limit = u64::try_from(MAX_PAYLOAD_BYTES + 1).unwrap_or(u64::MAX);
    stdin
        .take(limit)
        .read_to_end(&mut raw)
        .map_err(|error| Halt::internal(os_error_line(&error)))?;
    if raw.len() > MAX_PAYLOAD_BYTES {
        return Err(Halt::malformed("over 4 MiB"));
    }
    Ok(raw)
}

pub(crate) fn document(raw: &[u8]) -> Result<JsonValue, Halt> {
    let text =
        io::decode_strict(raw.to_vec()).map_err(|error| Halt::malformed(codec_text(raw, error)))?;
    json::parse(&text).map_err(|error| match error.kind {
        JsonErrorKind::ObjectTooDeep | JsonErrorKind::ArrayTooDeep => {
            Halt::internal(format!("RecursionError: {}", error.kind))
        }
        _ => Halt::malformed(error),
    })
}

pub(crate) fn identity(document: &JsonValue) -> Identity {
    let JsonValue::Object(fields) = document else {
        return Identity::default();
    };
    let text = |key: &str| match get(fields, key) {
        Some(JsonValue::Str(value)) => Some(lossy(value)),
        _ => None,
    };
    Identity {
        session_id: text("session_id"),
        transcript_path: text("transcript_path"),
        tool_use_id: text("tool_use_id"),
        tool_name: text("tool_name"),
    }
}

// The fields in CPython's order, so the first problem found is the one it reports.
pub(crate) fn payload(document: &JsonValue) -> Result<Payload, Halt> {
    let JsonValue::Object(fields) = document else {
        return Err(Halt::malformed("the payload is not a JSON object"));
    };
    let tool_name = string(fields, "tool_name", "payload")?;
    let is_write = match tool_name.as_str() {
        Some(WRITE) => true,
        Some(EDIT) => false,
        _ => {
            let shown = lossy(tool_name);
            return Err(Halt::skipped(format!("not a Write or Edit call: {shown}")));
        }
    };
    let input = object(fields, "tool_input", "payload")?;
    let file_path = file_path(fields, input)?;
    for key in ["session_id", "transcript_path", "tool_use_id"] {
        string(fields, key, "payload")?;
    }
    let change = if is_write {
        Change::Write(string(input, "content", "tool_input")?.clone())
    } else {
        Change::Edit(edit_input(input)?)
    };
    Ok(Payload { file_path, change })
}

fn file_path(fields: &Fields, input: &Fields) -> Result<PathBuf, Halt> {
    let given = path(
        string(input, "file_path", "tool_input")?,
        "tool_input.file_path",
    )?;
    if given.is_absolute() {
        return Ok(given.to_path_buf());
    }
    let cwd = path(string(fields, "cwd", "payload")?, "payload.cwd")?;
    Ok(cwd.join(given))
}

// Claude Code fills replace_all before the hook runs, but the model may omit it.
fn edit_input(input: &Fields) -> Result<EditInput, Halt> {
    let replace_all = match get(input, "replace_all") {
        Some(_) => boolean(input, "replace_all", "tool_input")?,
        None => false,
    };
    Ok(EditInput {
        old_string: string(input, "old_string", "tool_input")?.clone(),
        new_string: string(input, "new_string", "tool_input")?.clone(),
        replace_all,
    })
}

fn path<'a>(value: &'a CodePointString, place: &str) -> Result<&'a Path, Halt> {
    value
        .as_str()
        .map(Path::new)
        .ok_or_else(|| Halt::malformed(format!("{place} holds a lone surrogate")))
}

fn get<'a>(fields: &'a Fields, key: &str) -> Option<&'a JsonValue> {
    let key = CodePointString::from(key);
    fields
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
}

fn field<'a>(fields: &'a Fields, key: &str, place: &str) -> Result<&'a JsonValue, Halt> {
    get(fields, key).ok_or_else(|| Halt::malformed(format!("{place}.{key} is missing")))
}

fn mistyped(key: &str, place: &str, kind: &str) -> Halt {
    Halt::malformed(format!("{place}.{key} is not a {kind}"))
}

fn string<'a>(fields: &'a Fields, key: &str, place: &str) -> Result<&'a CodePointString, Halt> {
    match field(fields, key, place)? {
        JsonValue::Str(value) => Ok(value),
        _ => Err(mistyped(key, place, "str")),
    }
}

fn object<'a>(fields: &'a Fields, key: &str, place: &str) -> Result<&'a Fields, Halt> {
    match field(fields, key, place)? {
        JsonValue::Object(value) => Ok(value),
        _ => Err(mistyped(key, place, "dict")),
    }
}

fn boolean(fields: &Fields, key: &str, place: &str) -> Result<bool, Halt> {
    match field(fields, key, place)? {
        JsonValue::Bool(value) => Ok(*value),
        _ => Err(mistyped(key, place, "bool")),
    }
}

#[cfg(test)]
mod tests;
