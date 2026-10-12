//! The text Claude Code writes for a Write or an Edit, and the Edits that Claude Code itself fails.

use pycompat::PyStr;
use serde::Serialize;

use crate::halt::Halt;
use crate::payload::{Change, EditInput};

const NOT_IN_FILE: &str = "old_string is not in the file, so Claude Code fails this edit";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Eol {
    Crlf,
    Tie,
    Lf,
}

// The proposed text holds a lone surrogate, so it has no UTF-8 form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoneSurrogate;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub(crate) text: Result<String, LoneSurrogate>,
    pub(crate) eol: Option<Eol>,
}

pub(crate) fn proposal(change: &Change, old: Option<&str>) -> Result<Proposal, Halt> {
    match (change, old) {
        (Change::Write(content), _) => Ok(Proposal {
            text: utf8(content),
            eol: None,
        }),
        (Change::Edit(_), None) => Err(Halt::skipped(
            "Edit on a missing file, which Claude Code fails",
        )),
        (Change::Edit(edit), Some(old)) => edited_text(old, edit),
    }
}

pub(crate) fn edited_text(old: &str, edit: &EditInput) -> Result<Proposal, Halt> {
    let crlf = old.matches("\r\n").count();
    let lf = old.matches('\n').count() - crlf;
    let normalized = old.replace("\r\n", "\n");
    let needle = check_edit(&normalized, edit)?;
    let text = utf8(&edit.new_string).map(|new| {
        if edit.replace_all {
            normalized.replace(needle, &new)
        } else {
            normalized.replacen(needle, &new, 1)
        }
    });
    if crlf > lf {
        let text = text.map(|text| text.replace('\n', "\r\n"));
        return Ok(Proposal {
            text,
            eol: Some(Eol::Crlf),
        });
    }
    // A tie gets LF. Line endings change no metric, so this choice cannot move a finding.
    let eol = if crlf == lf && crlf > 0 {
        Eol::Tie
    } else {
        Eol::Lf
    };
    Ok(Proposal {
        text,
        eol: Some(eol),
    })
}

// The old_string to replace, or the reason Claude Code fails the Edit.
fn check_edit<'a>(normalized: &str, edit: &'a EditInput) -> Result<&'a str, Halt> {
    if edit.old_string == PyStr::default() {
        return Err(Halt::skipped(
            "Edit with an empty old_string, which Claude Code fails",
        ));
    }
    if edit.old_string == edit.new_string {
        return Err(Halt::skipped(
            "old_string equals new_string, which Claude Code fails",
        ));
    }
    // A lone surrogate never occurs in text that was read as UTF-8.
    let Some(needle) = edit.old_string.as_str() else {
        return Err(Halt::skipped(NOT_IN_FILE));
    };
    let occurrences = normalized.matches(needle).count();
    if occurrences == 0 {
        return Err(Halt::skipped(NOT_IN_FILE));
    }
    if occurrences > 1 && !edit.replace_all {
        return Err(Halt::skipped(format!(
            "old_string occurs {occurrences} times without replace_all, \
             so Claude Code fails this edit"
        )));
    }
    Ok(needle)
}

fn utf8(text: &PyStr) -> Result<String, LoneSurrogate> {
    text.as_str().map(str::to_owned).ok_or(LoneSurrogate)
}

#[cfg(test)]
mod tests;
