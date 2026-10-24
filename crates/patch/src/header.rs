//! The path that a block's header names: a path, nothing, or a line that shape cannot read.

use crate::quoted;

const FENCE: &str = "```";
const MAX_PATH_WORDS: usize = 3;
const MAX_EXTENSION: usize = 12;
// A line that ends on one of these is prose: sentence punctuation, or a closing delimiter.
const PROSE_END: &str = ".,:;!?\"')]}";
// A filename does not open with a delimiter, so a line that does is one this cannot read.
const PATH_START_REFUSED: &str = "\"'([{";

pub(crate) enum HeaderPath<'a> {
    Absent,
    Named(&'a str),
    Unreadable(String),
}

enum Shape {
    Path,
    Prose,
    Unreadable,
}

// The last path-shaped line wins. Fences and prose are skipped, and a line that is neither a path
// nor prose stops the walk, because the block below it belongs to whatever that line says.
pub(crate) fn filename_in(header: &str) -> HeaderPath<'_> {
    for line in text::splitlines(header, false).into_iter().rev() {
        let candidate = text::strip(line);
        if candidate.is_empty() || candidate.starts_with(FENCE) {
            continue;
        }
        let stripped = text::strip(candidate.trim_start_matches('#'));
        if stripped.is_empty() {
            continue;
        }
        match shape(stripped) {
            Shape::Path => return HeaderPath::Named(stripped),
            Shape::Prose => {}
            Shape::Unreadable => return HeaderPath::Unreadable(unreadable_reason(stripped)),
        }
    }
    HeaderPath::Absent
}

fn unreadable_reason(candidate: &str) -> String {
    format!(
        "the line above this block, {}, is neither a path nor a sentence, so which file this \
         block edits is not stated; put the path on its own line above <<<<<<< SEARCH",
        quoted(candidate)
    )
}

// One word is a path unless its punctuation says prose. Several words are a path only within a
// few words and with an extension on the last component.
fn shape(candidate: &str) -> Shape {
    if candidate.starts_with(|c| PATH_START_REFUSED.contains(c)) {
        return Shape::Unreadable;
    }
    if candidate.ends_with(|c| PROSE_END.contains(c)) {
        return Shape::Prose;
    }
    let words = text::split_whitespace(candidate).count();
    if words == 1 {
        return Shape::Path;
    }
    if words > MAX_PATH_WORDS || candidate.contains("  ") || candidate.contains('\t') {
        return Shape::Prose;
    }
    let name = candidate.rsplit('/').next().unwrap_or(candidate);
    if has_extension(name) {
        Shape::Path
    } else {
        Shape::Prose
    }
}

fn has_extension(name: &str) -> bool {
    let Some((stem, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty() && is_alphanumeric(suffix) && suffix.chars().count() <= MAX_EXTENSION
}

// A letter, digit or number character: the word class without the underscore.
fn is_alphanumeric(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c != '_' && text::is_word(c))
}
