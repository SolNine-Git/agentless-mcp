//! The matching keys of data files: after every code match, a card for each exact key, a few
//! cards or one line for the other matching keys of a file, and a count of the files the answer
//! does not list.

use std::collections::HashMap;

use facts::Symbol;

use super::match_rank;
use crate::scan::FileSymbols;
use crate::{SymbolCard, symbol_card};

// A file with more other matching keys than this shows one folded line instead of their cards.
const CARDS_PER_FILE: usize = 3;
// The most data entries an answer shows: a card and a folded file count one each.
pub(super) const DATA_ENTRIES: usize = 5;
// The match ranks of an exact name and of an exact qualified name.
const EXACT_RANKS: u8 = 1;

/// The matching keys of data files: the entries the answer shows, and the files it leaves out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataMatches {
    /// The cards and the folded files that the answer shows, exact keys first, in rank order.
    pub entries: Vec<DataEntry>,
    /// The files whose matching keys no entry shows, in rank order, each with the keys left out.
    pub unlisted: Vec<FoldedFile>,
    /// The most entries the answer has room for after the code cards.
    pub room: usize,
}

/// One shown data match: a key's card, or one line for a file with many matching keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataEntry {
    Card(SymbolCard),
    Folded(FoldedFile),
    /// The other matching keys of a file whose exact key has a card.
    FoldedRest(FoldedFile),
}

/// A data file and how many of its keys match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldedFile {
    /// The path below the repository root.
    pub path: String,
    /// The matching keys.
    pub keys: usize,
}

impl DataMatches {
    /// Returns how many data keys match, shown or not.
    #[must_use]
    pub fn total(&self) -> usize {
        let shown = self.entries.iter().map(|entry| match entry {
            DataEntry::Card(_) => 1,
            DataEntry::Folded(file) | DataEntry::FoldedRest(file) => file.keys,
        });
        shown.sum::<usize>() + self.unlisted.iter().map(|file| file.keys).sum::<usize>()
    }
}

// One matching key, its file, and whether its name or qualified name equals the query.
struct Ranked<'a> {
    file: &'a FileSymbols,
    symbol: &'a Symbol,
    exact: bool,
}

// One file's matching keys that are not exact, and the count of all its matching keys.
struct FileMatches<'a> {
    file: &'a FileSymbols,
    rest: Vec<&'a Symbol>,
    keys: usize,
}

// The exact keys take the first entries in rank order, then the other keys fill the room file by
// file. Every file after the first that does not fit is unlisted.
pub(super) fn data_matches(
    matches: Vec<(&FileSymbols, &Symbol)>,
    needle: &str,
    room: usize,
) -> DataMatches {
    let ranked = ranked(matches, needle);
    let exact = ranked.iter().filter(|key| key.exact).take(room);
    let mut shown: HashMap<&str, usize> = HashMap::new();
    let mut entries: Vec<DataEntry> = exact
        .map(|key| {
            *shown.entry(key.file.path.as_str()).or_default() += 1;
            DataEntry::Card(card(key.file, key.symbol))
        })
        .collect();
    let mut unlisted = Vec::new();
    let mut full = false;
    for group in by_file(&ranked) {
        let path = group.file.path.as_str();
        let mut seen = shown.get(path).copied().unwrap_or_default();
        let rest = rest_entries(&group, seen > 0);
        full = full || entries.len() + rest.len() > room;
        if !full {
            entries.extend(rest);
            seen += group.rest.len();
        }
        if seen < group.keys {
            let path = path.to_owned();
            unlisted.push(FoldedFile {
                path,
                keys: group.keys - seen,
            });
        }
    }
    DataMatches {
        entries,
        unlisted,
        room,
    }
}

// The matches ranked as the code matches are: rank, lowered name, path, then scan order.
fn ranked<'a>(matches: Vec<(&'a FileSymbols, &'a Symbol)>, needle: &str) -> Vec<Ranked<'a>> {
    let mut keyed: Vec<_> = (matches.into_iter().enumerate())
        .map(|(at, (file, symbol))| {
            let (rank, lowered) = match_rank(symbol, needle);
            ((rank, lowered, file.path.as_str(), at), file, symbol)
        })
        .collect();
    keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let ranked = keyed.into_iter().map(|(key, file, symbol)| Ranked {
        file,
        symbol,
        exact: key.0 <= EXACT_RANKS,
    });
    ranked.collect()
}

// The files in the rank order of their best match, each with its other keys in rank order.
fn by_file<'a>(ranked: &[Ranked<'a>]) -> Vec<FileMatches<'a>> {
    let mut groups: Vec<FileMatches<'a>> = Vec::new();
    let mut slots: HashMap<&str, usize> = HashMap::new();
    for key in ranked {
        let slot = *slots.entry(key.file.path.as_str()).or_insert_with(|| {
            let (file, rest) = (key.file, Vec::new());
            groups.push(FileMatches {
                file,
                rest,
                keys: 0,
            });
            groups.len() - 1
        });
        let group = &mut groups[slot];
        group.keys += 1;
        if !key.exact {
            group.rest.push(key.symbol);
        }
    }
    groups
}

// A card per other key, or one folded line when the file has many.
fn rest_entries(group: &FileMatches, beside_card: bool) -> Vec<DataEntry> {
    let (file, rest) = (group.file, &group.rest);
    if rest.len() > CARDS_PER_FILE {
        let folded = FoldedFile {
            path: file.path.clone(),
            keys: rest.len(),
        };
        let entry = if beside_card {
            DataEntry::FoldedRest(folded)
        } else {
            DataEntry::Folded(folded)
        };
        return vec![entry];
    }
    rest.iter()
        .map(|symbol| DataEntry::Card(card(file, symbol)))
        .collect()
}

fn card(file: &FileSymbols, symbol: &Symbol) -> SymbolCard {
    symbol_card(file.language, &file.path, symbol, String::new())
}
