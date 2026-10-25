//! The matching keys of data files: after every code match, a few cards, one line for a file
//! with many matching keys, and a count of the files the answer does not list.

use std::collections::HashMap;

use facts::Symbol;

use super::match_rank;
use crate::scan::FileSymbols;
use crate::{SymbolCard, symbol_card};

// A file with more matching keys than this shows one folded line instead of its cards.
const CARDS_PER_FILE: usize = 3;
// The most data entries an answer shows: a card and a folded file count one each.
pub(super) const DATA_ENTRIES: usize = 5;

/// The matching keys of data files: the entries the answer shows, and the files it leaves out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataMatches {
    /// The cards and the folded files that the answer shows, in rank order.
    pub entries: Vec<DataEntry>,
    /// The files whose matching keys no entry shows, in rank order.
    pub unlisted: Vec<FoldedFile>,
    /// Whether the scan holds a data file.
    pub scanned: bool,
}

/// One shown data match: a key's card, or one line for a file with many matching keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataEntry {
    Card(SymbolCard),
    Folded(FoldedFile),
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
            DataEntry::Folded(file) => file.keys,
        });
        shown.sum::<usize>() + self.unlisted.iter().map(|file| file.keys).sum::<usize>()
    }
}

// The files in the rank order of their best match. The entries fill `room` in that order, and
// every file after the first that does not fit is unlisted.
pub(super) fn data_matches(
    matches: Vec<(&FileSymbols, &Symbol)>,
    needle: &str,
    room: usize,
    scanned: bool,
) -> DataMatches {
    let mut data = DataMatches {
        scanned,
        ..DataMatches::default()
    };
    let mut full = false;
    for (file, symbols) in by_file(matches, needle) {
        let entries = file_entries(file, &symbols);
        full = full || data.entries.len() + entries.len() > room;
        if full {
            let (path, keys) = (file.path.clone(), symbols.len());
            data.unlisted.push(FoldedFile { path, keys });
        } else {
            data.entries.extend(entries);
        }
    }
    data
}

// The matches ranked as the code matches are, then grouped by file in the order of each file's
// best match.
fn by_file<'a>(
    matches: Vec<(&'a FileSymbols, &'a Symbol)>,
    needle: &str,
) -> Vec<(&'a FileSymbols, Vec<&'a Symbol>)> {
    let mut keyed: Vec<_> = (matches.into_iter().enumerate())
        .map(|(at, (file, symbol))| {
            let (rank, lowered) = match_rank(symbol, needle);
            ((rank, lowered, file.path.as_str(), at), file, symbol)
        })
        .collect();
    keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut groups: Vec<(&FileSymbols, Vec<&Symbol>)> = Vec::new();
    let mut slots: HashMap<&str, usize> = HashMap::new();
    for (_, file, symbol) in keyed {
        let slot = *slots.entry(file.path.as_str()).or_insert_with(|| {
            groups.push((file, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(symbol);
    }
    groups
}

fn file_entries(file: &FileSymbols, symbols: &[&Symbol]) -> Vec<DataEntry> {
    if symbols.len() > CARDS_PER_FILE {
        let (path, keys) = (file.path.clone(), symbols.len());
        return vec![DataEntry::Folded(FoldedFile { path, keys })];
    }
    let cards = symbols.iter().map(|symbol| {
        let card = symbol_card(file.language, &file.path, symbol, String::new());
        DataEntry::Card(card)
    });
    cards.collect()
}
