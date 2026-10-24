//! The edits loader: an edits.json document or SEARCH/REPLACE blocks, within the request bounds.

use json::{JsonValue, get};
use patch::{Edit, MAX_EDIT_BYTES, MAX_EDITS, ParseResult};
use text::CodePointString;

use crate::Error;

const EDITS_KEY: &str = "edits";
const INDEX_KEY: &str = "index";

/// Reads `text` as an edits.json document when its first non-space character is `{`, else as
/// SEARCH/REPLACE blocks. The document form reads every field strictly. Either form is refused
/// above [`MAX_EDITS`] edits or [`MAX_EDIT_BYTES`] bytes of search and replace text.
///
/// # Errors
///
/// Returns an operation failure for a document that is not a valid edits document, and for a
/// request over a bound.
pub fn load_edits(text: &str) -> Result<ParseResult, Error> {
    let parsed = if text.trim_start_matches(text::is_space).starts_with('{') {
        from_json(text)
    } else {
        Ok(patch::parse_blocks(text))
    };
    parsed
        .and_then(within_bounds)
        .map_err(|message| kernel::Error::OperationFailed(message).into())
}

fn from_json(text: &str) -> Result<ParseResult, String> {
    let document =
        json::parse(text).map_err(|error| format!("edits document is not valid JSON: {error}"))?;
    let entries = match &document {
        JsonValue::Object(pairs) => get(pairs, EDITS_KEY),
        _ => None,
    };
    let Some(entries) = entries else {
        return Err(format!(
            "edits document must be a JSON object with an '{EDITS_KEY}' list"
        ));
    };
    let JsonValue::Array(entries) = entries else {
        return Err(format!("'{EDITS_KEY}' must be a list of edit objects"));
    };
    let edits = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| edit_from(entry, position))
        .collect::<Result<Vec<Edit>, String>>()?;
    Ok(ParseResult {
        edits,
        errors: Vec::new(),
    })
}

// The three text fields first, in order, then the index. An entry without an index takes its
// position, because nothing keys on the index but the reports.
fn edit_from(entry: &JsonValue, position: usize) -> Result<Edit, String> {
    let JsonValue::Object(pairs) = entry else {
        return Err(format!("edit {position} is not a JSON object"));
    };
    let path = text_field(pairs, "path", position)?;
    let search = text_field(pairs, "search", position)?;
    let replace = text_field(pairs, "replace", position)?;
    let index = index_field(pairs, position)?;
    Ok(Edit {
        index,
        path,
        search,
        replace,
    })
}

// A str holds no lone surrogate, so a text that has one is refused here.
fn text_field(
    pairs: &[(CodePointString, JsonValue)],
    name: &str,
    position: usize,
) -> Result<String, String> {
    let Some(JsonValue::Str(value)) = get(pairs, name) else {
        return Err(format!("edit {position} is missing a string '{name}'"));
    };
    let text = value.as_str();
    let text = text.ok_or_else(|| format!("edit {position} has a lone surrogate in '{name}'"))?;
    Ok(text.to_owned())
}

// A bool is no integer, and an index must fit in 64 bits.
fn index_field(pairs: &[(CodePointString, JsonValue)], position: usize) -> Result<i64, String> {
    match get(pairs, INDEX_KEY) {
        None => i64::try_from(position).map_err(|_| out_of_range(position)),
        Some(JsonValue::Int(digits)) => digits.parse().map_err(|_| out_of_range(position)),
        Some(_) => Err(format!("edit {position} has a non-integer '{INDEX_KEY}'")),
    }
}

fn out_of_range(position: usize) -> String {
    format!("edit {position} has an '{INDEX_KEY}' outside the 64-bit integer range")
}

fn within_bounds(parsed: ParseResult) -> Result<ParseResult, String> {
    let count = parsed.edits.len();
    if count > MAX_EDITS {
        return Err(format!(
            "the request carries {count} edits, more than the {MAX_EDITS} this tool applies at \
             once"
        ));
    }
    let total: usize = parsed
        .edits
        .iter()
        .map(|edit| edit.search.len() + edit.replace.len())
        .sum();
    if total > MAX_EDIT_BYTES {
        return Err(format!(
            "the request carries {total} bytes of search and replace text, more than the \
             {MAX_EDIT_BYTES} this tool applies at once"
        ));
    }
    Ok(parsed)
}
