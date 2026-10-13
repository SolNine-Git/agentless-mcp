//! Symbols by a substring or a qualified name, ranked exact name first, with the places a missed
//! name is spelled where a file did not parse.

use std::collections::BTreeSet;
use std::path::Path;

use facts::{Symbol, SymbolKind};
use graph::joins_name_graph;
use kernel::Bounded;
use store::{FileText, Served};
use text::lower;

use crate::bounds::within;
use crate::ops::limits::MAX_LIMIT;
use crate::scan::{FileSymbols, SymbolFacts};
use crate::{Error, IntArg, Noted, Reading, Services, Skipped, SymbolCard, symbol_card};

/// A find-symbol request: the name or part of a name, the kind filter, and the most cards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindRequest {
    /// The text that a name or a qualified name holds, matched case-insensitively.
    pub query: String,
    /// The one kind a match must have, or `None` for any kind.
    pub kind: Option<SymbolKind>,
    /// The most cards the answer lists. The count is always complete.
    pub limit: IntArg,
}

/// The cards of the matching symbols, and what the answer cannot vouch for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindResult {
    /// The query as the caller wrote it.
    pub query: String,
    /// One card per match the limit kept, and how many matched.
    pub cards: Bounded<SymbolCard>,
    /// The files the scan passed over.
    pub skipped: Vec<Skipped>,
    /// The kind filter's name, if any.
    pub kind: Option<&'static str>,
    /// On a miss that the kind filter caused, the kinds of the symbols the query matches.
    pub other_kinds: Vec<&'static str>,
    /// On a miss, each place the query is spelled where a file did not parse.
    pub unparsed: Vec<(String, u32)>,
}

/// Finds the symbols of the repository that `reading` names whose name or qualified name holds
/// the query. The scan's warnings come with either outcome.
#[must_use]
pub fn find(
    services: &Services,
    reading: &Reading<'_>,
    request: &FindRequest,
) -> Noted<Result<FindResult, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    let loaded = SymbolFacts::load(services, reading);
    let mut diagnostics = loaded.diagnostics;
    let scan = match loaded.value {
        Ok(scan) => scan,
        Err(error) => {
            return Noted {
                value: Err(error),
                diagnostics,
            };
        }
    };
    let found = matched(&scan, request, limit);
    if found.cards.total() > 0 {
        return Noted {
            value: Ok(found),
            diagnostics,
        };
    }
    let unparsed = unparsed_mentions(reading, &scan, &request.query);
    diagnostics.extend(unparsed.diagnostics);
    let value = unparsed
        .value
        .map(|unparsed| FindResult { unparsed, ..found });
    Noted { value, diagnostics }
}

// The result without the unparsed places, which only a miss reads.
fn matched(scan: &SymbolFacts, request: &FindRequest, limit: usize) -> FindResult {
    let needle = lower(&request.query);
    let symbols = scan.files.iter().flat_map(|file| {
        let symbols = file.symbols.iter();
        symbols.map(move |symbol| (file, symbol))
    });
    let mut matches: Vec<(&FileSymbols, &Symbol)> = symbols
        .filter(|(_, symbol)| matches(symbol, &needle, request.kind))
        .collect();
    matches.sort_by_cached_key(|&(file, symbol)| {
        let (rank, lowered) = match_rank(symbol, &needle);
        (rank, lowered, file.path.as_str())
    });
    let cards: Vec<SymbolCard> = matches
        .iter()
        .map(|(file, symbol)| symbol_card(file.language, &file.path, symbol, String::new()))
        .collect();
    let other_kinds = match request.kind {
        Some(_) if matches.is_empty() => other_kinds(scan, &needle),
        _ => Vec::new(),
    };
    FindResult {
        query: request.query.clone(),
        cards: Bounded::cut(cards, limit),
        skipped: scan.skipped.clone(),
        kind: request.kind.map(SymbolKind::as_str),
        other_kinds,
        unparsed: Vec::new(),
    }
}

// The kinds, sorted, of the symbols that the query matches with no kind filter.
fn other_kinds(scan: &SymbolFacts, needle: &str) -> Vec<&'static str> {
    let symbols = scan.files.iter().flat_map(|file| file.symbols.iter());
    let kinds: BTreeSet<&'static str> = symbols
        .filter(|symbol| matches(symbol, needle, None))
        .map(|symbol| symbol.kind.as_str())
        .collect();
    kinds.into_iter().collect()
}

fn matches(symbol: &Symbol, needle: &str, kind: Option<SymbolKind>) -> bool {
    if kind.is_some_and(|kind| kind != symbol.kind) {
        return false;
    }
    lower(&symbol.name).contains(needle) || lower(&symbol.qualname()).contains(needle)
}

// An exact name first, then an exact qualified name, then a name that starts with the query.
fn match_rank(symbol: &Symbol, needle: &str) -> (u8, String) {
    let lowered = lower(&symbol.name);
    let rank = if lowered == needle {
        0
    } else if lower(&symbol.qualname()) == needle {
        1
    } else if lowered.starts_with(needle) {
        2
    } else {
        3
    };
    (rank, lowered)
}

// Only the files that spell the query are parsed again: grammars leave error nodes in much
// valid code, so a blanket parse-error flag is noise.
fn unparsed_mentions(
    reading: &Reading<'_>,
    scan: &SymbolFacts,
    query: &str,
) -> Noted<Result<Vec<(String, u32)>, Error>> {
    let needle = lower(query);
    let mut found = Noted::new(Vec::new());
    if needle.is_empty() {
        return found.map(Ok);
    }
    for file in &scan.files {
        if !joins_name_graph(file.language) {
            continue;
        }
        let mentions = match mentions_in(reading, file, &needle, query) {
            Ok(mentions) => mentions,
            Err(error) => return found.map(|_| Err(error)),
        };
        found.diagnostics.extend(mentions.diagnostics);
        let places = mentions
            .value
            .into_iter()
            .map(|line| (file.path.clone(), line));
        found.value.extend(places);
    }
    found.map(Ok)
}

// The lines of one file where the query is spelled inside a region that did not parse.
fn mentions_in(
    reading: &Reading<'_>,
    file: &FileSymbols,
    needle: &str,
    query: &str,
) -> Result<Noted<Vec<u32>>, Error> {
    let absolute = io::contained_path(&reading.repo.root, Path::new(&file.path))
        .map_err(|refusal| kernel::Error::SecurityRefusal(refusal.to_string()))?;
    let Ok(bytes) = io::read_bounded(&absolute) else {
        return Ok(Noted::new(Vec::new()));
    };
    let text = FileText::decode(file.path.clone(), file.language, &bytes);
    if !lower(&text.text).contains(needle) {
        return Ok(Noted::new(Vec::new()));
    }
    let Served { value, warning } = reading.facts.facts(&text);
    let lines = value.map_or_else(
        |_| Vec::new(),
        |facts| facts::unparsed_mentions(&text.text, &facts.errors, query),
    );
    let diagnostics = warning.map(|failure| failure.to_string());
    Ok(Noted {
        value: lines,
        diagnostics: diagnostics.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests;
