//! The code files that spell a queried name as a whole word and declare no symbol of that name:
//! the files nearest to a declaration first, cut to a few, with a count of the rest.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use graph::joins_name_graph;
use kernel::Bounded;
use store::FileText;

use crate::ops::limits::FIND_USAGE_FILES;
use crate::scan::{FileSymbols, Spelling, SymbolFacts};

/// The code files that spell a name as a word and do not declare it, in the order they come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    /// The name as the query writes it: the last segment of a qualified name.
    pub name: String,
    /// The segment before the name in a qualified query, which every listed file also spells.
    pub owner: Option<String>,
    /// The order of the files.
    pub order: UsageOrder,
    /// The files the cap keeps, in order, and how many files spell the name.
    pub files: Bounded<UsageFile>,
}

/// The order of the usage files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageOrder {
    /// The fewest directory steps to a declaring file first, then more lines, then the path.
    NearestDeclaration,
    /// No file declares the name: more lines first, then the path.
    MostLines,
}

/// One code file that spells the name: its path, its first such line, and how many lines do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageFile {
    /// The path below the repository root.
    pub path: String,
    /// The first line that spells the name.
    pub line: u32,
    /// How many lines spell the name.
    pub lines: u32,
}

// The name of a query, and the owner that a qualified query writes before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UsageQuery<'q> {
    pub(super) name: &'q str,
    pub(super) owner: Option<&'q str>,
}

impl<'q> UsageQuery<'q> {
    // A name, or names joined by `.` or `::`. Any other query, the empty one too, has no usage.
    pub(super) fn of(query: &'q str) -> Option<Self> {
        let segments: Vec<&str> = (query.split("::"))
            .flat_map(|part| part.split('.'))
            .collect();
        let named = |segment: &&str| !segment.is_empty() && segment.chars().all(is_word);
        if !segments.iter().all(named) {
            return None;
        }
        let (name, owners) = segments.split_last()?;
        let owner = owners.last().copied();
        Some(Self { name, owner })
    }
}

// The words that the queries ask the scan to spell: each name, then its owner, each word once.
pub(super) fn words<'q>(queries: &[UsageQuery<'q>]) -> Vec<&'q str> {
    let mut words: Vec<&str> = Vec::new();
    for word in queries
        .iter()
        .flat_map(|query| [Some(query.name), query.owner])
    {
        if let Some(word) = word.filter(|word| !words.contains(word)) {
            words.push(word);
        }
    }
    words
}

// Where a code file spells each word. A data file spells none, so it is never a usage file.
pub(super) fn spellings(text: &FileText, words: &[&str]) -> Vec<Option<Spelling>> {
    if !joins_name_graph(text.language) {
        return Vec::new();
    }
    words.iter().map(|word| spelled(&text.text, word)).collect()
}

// The first line that spells `word` with no word character on either side, and how many lines do.
// Each character of a name is a word character, so a match that overlaps a match is never whole.
pub(super) fn spelled(text: &str, word: &str) -> Option<Spelling> {
    let mut found: Option<Spelling> = None;
    let (mut line, mut from, mut counted) = (1_u32, 0, 0_u32);
    for (at, _) in text.match_indices(word) {
        if !stands_alone(text, at, word.len()) {
            continue;
        }
        let breaks = text[from..at].bytes().filter(|byte| *byte == b'\n').count();
        line = line.saturating_add(u32::try_from(breaks).unwrap_or(u32::MAX));
        from = at;
        if line == counted {
            continue;
        }
        counted = line;
        found.get_or_insert(Spelling { line, lines: 0 }).lines += 1;
    }
    found
}

fn stands_alone(text: &str, at: usize, length: usize) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + length..].chars().next();
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

// A letter, a digit or other number, `_`, or the `$` that JavaScript names hold.
fn is_word(c: char) -> bool {
    c == '$' || text::is_word(c)
}

// What a usage file must be: where the scan's words put the name and the owner, the name, and
// the directories of the files that declare it.
struct Rule<'a> {
    name: &'a str,
    name_at: usize,
    owner_at: Option<usize>,
    declaring: Vec<Vec<&'a str>>,
}

// The usage files of one query, or `None` when no code file spells the name and leaves it
// undeclared. `words` is the word order that the scan's spellings follow.
pub(super) fn usage(scan: &SymbolFacts, words: &[&str], query: &UsageQuery<'_>) -> Option<Usage> {
    let at = |word: &str| words.iter().position(|listed| *listed == word);
    let owner_at = match query.owner {
        Some(owner) => Some(at(owner)?),
        None => None,
    };
    let rule = Rule {
        name: query.name,
        name_at: at(query.name)?,
        owner_at,
        declaring: declaring(scan, query.name),
    };
    let mut ranked: Vec<_> = (scan.files.iter())
        .filter_map(|file| ranked_file(file, &rule))
        .collect();
    if ranked.is_empty() {
        return None;
    }
    ranked.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let order = if rule.declaring.is_empty() {
        UsageOrder::MostLines
    } else {
        UsageOrder::NearestDeclaration
    };
    let files = ranked.into_iter().map(|(_, file)| file).collect();
    Some(Usage {
        name: query.name.to_owned(),
        owner: query.owner.map(str::to_owned),
        order,
        files: Bounded::cut(files, FIND_USAGE_FILES),
    })
}

// The directories of the scanned files, data files too, that declare a symbol named `name` in
// its case, each once.
fn declaring<'s>(scan: &'s SymbolFacts, name: &str) -> Vec<Vec<&'s str>> {
    let files = scan.files.iter().filter(|file| declares(file, name));
    let directories: BTreeSet<Vec<&str>> = files.map(|file| directories(&file.path)).collect();
    directories.into_iter().collect()
}

fn declares(file: &FileSymbols, name: &str) -> bool {
    file.symbols.iter().any(|symbol| symbol.name == name)
}

type Key<'f> = (usize, Reverse<u32>, &'f str);

// A file's order key and row, when it spells the name and the owner and does not declare the
// name. With no declaring file, every distance is 0.
fn ranked_file<'f>(file: &'f FileSymbols, rule: &Rule<'_>) -> Option<(Key<'f>, UsageFile)> {
    let spelled = |at: usize| file.spellings.get(at).copied().flatten();
    let spelling = spelled(rule.name_at)?;
    let owner_missing = rule.owner_at.is_some_and(|at| spelled(at).is_none());
    if owner_missing || declares(file, rule.name) {
        return None;
    }
    let from = directories(&file.path);
    let nearest = (rule.declaring.iter())
        .map(|to| steps(&from, to))
        .min()
        .unwrap_or(0);
    let key = (nearest, Reverse(spelling.lines), file.path.as_str());
    let row = UsageFile {
        path: file.path.clone(),
        line: spelling.line,
        lines: spelling.lines,
    };
    Some((key, row))
}

fn directories(path: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts
}

// The directory steps from one directory up to the deepest directory that holds both, then down.
fn steps(from: &[&str], to: &[&str]) -> usize {
    let common = from.iter().zip(to).take_while(|(a, b)| a == b).count();
    from.len() + to.len() - 2 * common
}

#[cfg(test)]
mod tests;
