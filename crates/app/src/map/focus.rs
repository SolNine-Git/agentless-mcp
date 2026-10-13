//! What every view means by a focus entry: the files it names, as a path, a module stem, a symbol,
//! a path spelled inside a traceback, a URL or `path:line`, or a name a few files spell.

use std::collections::{BTreeMap, BTreeSet};

use graph::{Definition, NameIndex, Site};
use text::{is_decimal, is_space, strip};

use crate::scope::is_test_path;

// Past this many non-test files a name is the repository's vocabulary, not a locator.
const SPELLING_MAX_FILES: usize = 5;
// The bound on the tail walk keeps a pathological entry from scanning the files thousands of times.
const MAX_TAIL_COMPONENTS: usize = 32;

// The paths of one scan.
pub(crate) type Known<'a> = BTreeSet<&'a str>;

// The files `entry` names, sorted or by spelling count, and the definitions when it named a
// symbol. A path shape wins over a symbol; the spellings inside the entry retry only the paths.
pub(crate) fn focus_resolution<'a>(
    entry: &str,
    known: &Known<'_>,
    index: &NameIndex<'a>,
) -> (Vec<String>, Vec<Definition<'a>>) {
    let paths = path_matches(&pure_posix(entry), known);
    if !paths.is_empty() {
        return (paths, Vec::new());
    }
    let qualified = match entry.rsplit_once("::") {
        Some((_, after)) if !after.is_empty() => after,
        _ => entry,
    };
    let name = bare_name(qualified);
    if let Some(found) = name.and_then(|name| symbol_entry(qualified, name, known, index)) {
        return found;
    }
    for candidate in normalized_spellings(entry) {
        let paths = path_matches(&candidate, known);
        if !paths.is_empty() {
            return (paths, Vec::new());
        }
    }
    let spelled = name.map(|_| spelling_matches(qualified, known, index));
    (spelled.unwrap_or_default(), Vec::new())
}

// A path-shaped entry names no symbol: the text after its last dot is an extension.
fn bare_name(qualified: &str) -> Option<&str> {
    if qualified.contains('/') {
        return None;
    }
    match qualified.rsplit_once('.') {
        Some((_, last)) if !last.is_empty() => Some(last),
        _ => Some(qualified),
    }
}

fn symbol_entry<'a>(
    qualified: &str,
    name: &str,
    known: &Known<'_>,
    index: &NameIndex<'a>,
) -> Option<(Vec<String>, Vec<Definition<'a>>)> {
    let defining: Vec<Definition<'a>> = (index.definitions(name).iter())
        .filter(|definition| known.contains(definition.path()))
        .copied()
        .collect();
    let owned: Vec<Definition<'a>> = (defining.iter())
        .filter(|definition| definition.symbol().qualname() == qualified)
        .copied()
        .collect();
    let matched = if owned.is_empty() { defining } else { owned };
    if matched.is_empty() {
        return None;
    }
    let paths: BTreeSet<&str> = matched.iter().map(Definition::path).collect();
    Some((paths.into_iter().map(str::to_owned).collect(), matched))
}

fn path_matches(normalized: &str, known: &Known<'_>) -> Vec<String> {
    if known.contains(normalized) {
        return vec![normalized.to_owned()];
    }
    let suffix = format!("/{normalized}");
    let suffixed: Vec<String> = (known.iter())
        .filter(|path| path.ends_with(&suffix))
        .map(|path| (*path).to_owned())
        .collect();
    if !suffixed.is_empty() {
        return suffixed;
    }
    let stems = known.iter().filter(|path| matches_stem(path, normalized));
    stems.map(|path| (*path).to_owned()).collect()
}

fn matches_stem(path: &str, entry: &str) -> bool {
    let bare = without_suffix(path);
    bare == entry
        || bare
            .strip_suffix(entry)
            .is_some_and(|head| head.ends_with('/'))
}

fn without_suffix(path: &str) -> &str {
    let start = path.rfind('/').map_or(0, |at| at + 1);
    let name = &path[start..];
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => &path[..start + dot],
        _ => path,
    }
}

fn pure_posix(text: &str) -> String {
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = (text.split('/'))
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if root.is_empty() && parts.is_empty() {
        return ".".to_owned();
    }
    format!("{root}{}", parts.join("/"))
}

// A dotted entry is evidence only where a file spells every part of it.
fn spelling_matches(qualified: &str, known: &Known<'_>, index: &NameIndex<'_>) -> Vec<String> {
    let mut parts: Vec<&str> = qualified.split('.').collect();
    let name = parts.pop().unwrap_or_default();
    if name.is_empty() || parts.iter().any(|owner| owner.is_empty()) {
        return Vec::new();
    }
    let Some(mut counts) = spelling_counts(name, known, index) else {
        return Vec::new();
    };
    for owner in parts {
        let spelling: BTreeSet<&str> = index.sites(owner).iter().map(Site::path).collect();
        counts.retain(|path, _| spelling.contains(path));
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    ranked
        .into_iter()
        .map(|(path, _)| path.to_owned())
        .collect()
}

// Tests enter the ranking as pure sources, so a seed that reaches only tests ranks nothing.
fn spelling_counts<'i>(
    name: &str,
    known: &Known<'_>,
    index: &'i NameIndex<'_>,
) -> Option<BTreeMap<&'i str, usize>> {
    let mut counts = BTreeMap::new();
    for site in index.sites(name) {
        let path = site.path();
        if !known.contains(path) || is_test_path(path) {
            continue;
        }
        *counts.entry(path).or_insert(0) += 1;
        if counts.len() > SPELLING_MAX_FILES {
            return None;
        }
    }
    Some(counts)
}

// Longest first and never the entry: the frame or URL path without its location, then its tails.
fn normalized_spellings(entry: &str) -> Vec<String> {
    let core = match frame_path(entry) {
        Some(path) => path.to_owned(),
        None => without_scheme(entry).map_or_else(|| entry.to_owned(), without_query),
    };
    let core = strip(&without_location(&core)).replace('\\', "/");
    let mut candidates = Vec::new();
    if !core.is_empty() && core != entry {
        candidates.push(pure_posix(&core));
    }
    let parts: Vec<&str> = (core.split('/'))
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let parts = &parts[parts.len().saturating_sub(MAX_TAIL_COMPONENTS)..];
    candidates.extend((1..parts.len()).map(|start| parts[start..].join("/")));
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| candidate != entry && seen.insert(candidate.clone()));
    candidates
}

fn without_query(rest: &str) -> String {
    let rest = rest.split_once('?').map_or(rest, |(head, _)| head);
    rest.split_once('#')
        .map_or(rest, |(head, _)| head)
        .to_owned()
}

// The path of `\s*File "<path>", line <digits>` at the start of `entry`.
fn frame_path(entry: &str) -> Option<&str> {
    let rest = entry.trim_start_matches(is_space).strip_prefix("File \"")?;
    let end = rest.find('"').filter(|&end| end > 0)?;
    let after = rest[end..].strip_prefix("\", line ")?;
    after.chars().next().filter(|&first| is_decimal(first))?;
    Some(&rest[..end])
}

// The entry after a leading `<scheme>://`.
fn without_scheme(entry: &str) -> Option<&str> {
    let bytes = entry.as_bytes();
    if !bytes.first()?.is_ascii_alphabetic() {
        return None;
    }
    let scheme = (bytes[1..].iter())
        .take_while(|&&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
        .count();
    entry[1 + scheme..].strip_prefix("://")
}

// The leftmost `:N`, `:N-M`, `#LN` or `#LN-[L]M` that ends the text or precedes its last line feed.
fn without_location(core: &str) -> String {
    for (at, _) in core.char_indices() {
        if let Some(rest) = location_tail(&core[at..]) {
            return format!("{}{rest}", &core[..at]);
        }
    }
    core.to_owned()
}

fn location_tail(text: &str) -> Option<&str> {
    let after = if let Some(rest) = text.strip_prefix(':') {
        line_range(rest, "")?
    } else {
        line_range(text.strip_prefix("#L")?, "L")?
    };
    (after.is_empty() || after == "\n").then_some(after)
}

// Digits, then optionally `-`, `marker` when it is there, and digits; the text after them.
fn line_range<'t>(text: &'t str, marker: &str) -> Option<&'t str> {
    let rest = digits(text)?;
    let end = rest
        .strip_prefix('-')
        .map(|tail| tail.strip_prefix(marker).unwrap_or(tail));
    Some(end.and_then(digits).unwrap_or(rest))
}

fn digits(text: &str) -> Option<&str> {
    let end = (text.char_indices())
        .find(|&(_, c)| !is_decimal(c))
        .map_or(text.len(), |(at, _)| at);
    (end > 0).then(|| &text[end..])
}

#[cfg(test)]
mod tests;
