//! Equivalence keys: a sha256 over the line diff of two normalized token streams.

use std::collections::BTreeMap;
use std::sync::Arc;

use kernel::Key;
use sha2::{Digest, Sha256};
use syntax::{Language, Unparsable};

/// The prefix of a key that no grammar stands behind: its stream is whitespace-collapsed text.
pub const TEXT_ONLY_KEY_PREFIX: &str = "text:";

/// Returns the normalized token stream of `text`: every leaf with directive comments kept and
/// other comments dropped, joined by single spaces. Without a language it returns the text with
/// its whitespace collapsed.
///
/// # Errors
///
/// Returns the parser's refusal of `text`.
pub fn normalized_stream(text: &str, language: Option<Language>) -> Result<String, Unparsable> {
    Ok(stream(text, language)?.0)
}

/// Returns the equivalence key of one file's change from `old` to `new`. A comment-only or
/// whitespace-only change gives the key of no change.
///
/// # Errors
///
/// Returns the parser's refusal of either text.
pub fn file_key(old: &str, new: &str, language: Option<Language>) -> Result<String, Unparsable> {
    let (before, grammar) = stream(old, language)?;
    let (after, _) = stream(new, language)?;
    let diff = text::unified_diff(
        &text::splitlines(&before, false),
        &text::splitlines(&after, false),
    )
    .join("\n");
    Ok(labelled(&hex_sha256(diff.as_bytes()), grammar))
}

/// Returns the equivalence key of a patch from the key of each file it changes, by path. The
/// key carries the text-only prefix when any one file's key does.
#[must_use]
pub fn equivalence_key(file_keys: &BTreeMap<String, String>) -> String {
    let mut digest = Sha256::new();
    let mut degraded = false;
    for (path, key) in file_keys {
        degraded |= key.starts_with(TEXT_ONLY_KEY_PREFIX);
        // The path keeps one edit made to two different files from hashing to one key.
        digest.update(path);
        digest.update(b"\0");
        digest.update(key);
        digest.update(b"\n");
    }
    labelled(&Key::new(digest.finalize().into()).to_string(), !degraded)
}

// The stream, and whether a grammar produced it.
fn stream(text: &str, language: Option<Language>) -> Result<(String, bool), Unparsable> {
    let Some(language) = language else {
        let words: Vec<&str> = text::split_whitespace(text).collect();
        return Ok((words.join(" "), false));
    };
    let parsed = syntax::parse(language, Arc::from(text))?;
    Ok((syntax::tokens(&parsed, parsed.root(), None).join(" "), true))
}

fn hex_sha256(bytes: &[u8]) -> String {
    Key::new(Sha256::digest(bytes).into()).to_string()
}

fn labelled(digest: &str, grammar: bool) -> String {
    if grammar {
        digest.to_owned()
    } else {
        format!("{TEXT_ONLY_KEY_PREFIX}{digest}")
    }
}
