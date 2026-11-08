//! The seeds of a prompt and the context sentence, as the measured bench prototype finds and
//! writes them: rule P with a cap of 2 files.

use std::collections::HashSet;

use prompts::messages::PROMPT_MAP_CONTEXT;
use prompts::{Template, Text};
use text::{is_word, strip};

const MAX_SEEDS: usize = 12;
// In code points, as Python counts a string.
const MIN_TOKEN: usize = 4;
const MAX_TICKED: usize = 120;
const MAX_SUFFIX: usize = 6;
pub(crate) const MAX_FILES: usize = 2;
const SHOWN_NAMES: usize = 4;
pub(crate) const MAX_CONTEXT_BYTES: usize = 768;

// The seeds of a text, and the repository files that it names, in order.
pub(crate) struct Seeded {
    pub(crate) seeds: Vec<String>,
    pub(crate) files: Vec<String>,
}

pub(crate) fn seeds(prompt: &str, is_file: &dyn Fn(&str) -> bool) -> Vec<String> {
    seeded(prompt, is_file).seeds
}

pub(crate) fn seeded(prompt: &str, is_file: &dyn Fn(&str) -> bool) -> Seeded {
    let file = |rel: &str| below_root(rel) && is_file(rel);
    let mut paths: Vec<&str> = path_tokens(prompt)
        .into_iter()
        .map(|token| token.trim_start_matches('/'))
        .filter(|rel| file(rel))
        .collect();
    let mut ticked = Vec::new();
    for raw in backticked(prompt) {
        let token = strip(raw);
        let token = token.strip_suffix("()").unwrap_or(token);
        if token.chars().count() < MIN_TOKEN {
            continue;
        }
        let rel = token.trim_start_matches('/');
        if file(rel) {
            paths.push(rel);
        } else if is_dotted_name(token) {
            ticked.push(token);
        }
    }
    let mut named = HashSet::new();
    let files = (paths.iter())
        .filter(|path| named.insert(**path))
        .map(|path| (*path).to_owned())
        .collect();
    let words = [camel_case(prompt), snake_case(prompt)]
        .into_iter()
        .flatten();
    let words = words.filter(|word| word.len() >= MIN_TOKEN);
    let mut seen = HashSet::new();
    let seeds = (paths.into_iter().chain(ticked).chain(words))
        .filter(|seed| seen.insert(*seed))
        .take(MAX_SEEDS)
        .map(str::to_owned)
        .collect();
    Seeded { seeds, files }
}

pub(crate) fn context_text(names: &[String], files: &[String]) -> Option<String> {
    sentence(&PROMPT_MAP_CONTEXT, names, files)
}

// The names, then the files, in the sentence of `template`: two files, or one when two make the
// sentence too long.
pub(crate) fn sentence(
    template: &Template<(Text, Text)>,
    names: &[String],
    files: &[String],
) -> Option<String> {
    if names.is_empty() || files.is_empty() {
        return None;
    }
    let names = joined(names, SHOWN_NAMES, ", ");
    [MAX_FILES, 1]
        .into_iter()
        .map(|kept| template.render((joined(files, kept, " and ").as_str(), &names)))
        .find(|text| text.len() <= MAX_CONTEXT_BYTES)
}

fn joined(items: &[String], cap: usize, separator: &str) -> String {
    let kept: Vec<&str> = items.iter().take(cap).map(String::as_str).collect();
    kept.join(separator)
}

// No segment is empty, `.` or `..`, so the path stays below the root.
fn below_root(rel: &str) -> bool {
    rel.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'/')
}

// The matches of [A-Za-z0-9_.\-/]+\.[A-Za-z0-9]{1,6} in Python's order. A start that matches
// nothing leaves no match at a later start in the same run of path bytes.
fn path_tokens(prompt: &str) -> Vec<&str> {
    let bytes = prompt.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let run = bytes[at..]
            .iter()
            .take_while(|&&byte| is_path_byte(byte))
            .count();
        if run == 0 {
            at += 1;
            continue;
        }
        match path_end(&bytes[at..at + run]) {
            Some(end) => {
                found.push(&prompt[at..at + end]);
                at += end;
            }
            None => at += run,
        }
    }
    found
}

// The greedy match ends after the last dot that has a byte before it and an alphanumeric byte
// after it, and takes at most 6 alphanumeric bytes after that dot.
fn path_end(run: &[u8]) -> Option<usize> {
    let dot = (1..run.len())
        .rev()
        .find(|&at| run[at] == b'.' && run.get(at + 1).is_some_and(u8::is_ascii_alphanumeric))?;
    let suffix = run[dot + 1..]
        .iter()
        .take(MAX_SUFFIX)
        .take_while(|byte| byte.is_ascii_alphanumeric())
        .count();
    Some(dot + 1 + suffix)
}

// The groups of `([^`\n]{1,120})`. A backtick that opens no match is passed over by one
// character, so it can close nothing.
fn backticked(prompt: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = prompt;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let inner = after.find(['`', '\n']).unwrap_or(after.len());
        let body = &after[..inner];
        let length = body.chars().count();
        if after[inner..].starts_with('`') && (1..=MAX_TICKED).contains(&length) {
            found.push(body);
            rest = &after[inner + 1..];
        } else {
            rest = after;
        }
    }
    found
}

// \b[A-Z][a-z0-9]+(?:[A-Z][a-z0-9]*)+\b
fn camel_case(prompt: &str) -> Vec<&str> {
    whole_words(prompt, |byte| byte.is_ascii_alphanumeric(), is_camel)
}

// \b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b
fn snake_case(prompt: &str) -> Vec<&str> {
    let class = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_';
    whole_words(prompt, class, is_snake)
}

fn is_camel(word: &[u8]) -> bool {
    matches!(word, [first, second, rest @ ..]
        if first.is_ascii_uppercase()
            && !second.is_ascii_uppercase()
            && rest.iter().any(u8::is_ascii_uppercase))
}

fn is_snake(word: &[u8]) -> bool {
    word.first().is_some_and(u8::is_ascii_lowercase)
        && word.contains(&b'_')
        && word
            .split(|&byte| byte == b'_')
            .all(|part| !part.is_empty())
}

// Each class byte is a word character, so a match between two word boundaries is a whole run
// of class bytes. The boundaries use Python's Unicode word class.
fn whole_words(prompt: &str, class: fn(u8) -> bool, shape: fn(&[u8]) -> bool) -> Vec<&str> {
    let bytes = prompt.as_bytes();
    let mut found = Vec::new();
    let mut before: Option<char> = None;
    let mut at = 0;
    while let Some(next) = prompt[at..].chars().next() {
        let opens = before.is_none_or(|before| !is_word(before));
        if !(opens && u8::try_from(next).is_ok_and(class)) {
            before = Some(next);
            at += next.len_utf8();
            continue;
        }
        let end = at + bytes[at..].iter().take_while(|&&byte| class(byte)).count();
        let closes = prompt[end..]
            .chars()
            .next()
            .is_none_or(|after| !is_word(after));
        if closes && shape(&bytes[at..end]) {
            found.push(&prompt[at..end]);
        }
        before = Some(char::from(bytes[end - 1]));
        at = end;
    }
    found
}

// ^[A-Za-z_][A-Za-z0-9_.]*$
fn is_dotted_name(token: &str) -> bool {
    let mut bytes = token.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
}
