//! Symbols by a substring or a qualified name, ranked exact name first, with the places a missed
//! name is spelled where a file did not parse or where a data file spells it outside its outline.

use std::collections::BTreeSet;
use std::path::Path;

mod data;
mod outside;

use facts::{Symbol, SymbolKind, is_data_language};
use graph::joins_name_graph;
use kernel::{Bounded, checkpoint};
use store::{FileText, Served};
use text::lower;

use crate::bounds::within;
use crate::detached::answered_over;
use crate::ops::limits::MAX_LIMIT;
use crate::scan::{FileSymbols, SymbolFacts};
use crate::{Error, IntArg, Noted, Reading, Services, Skipped, SymbolCard, symbol_card};
use data::{DATA_ENTRIES, data_matches};
use outside::outside_outline;

pub use data::{DataEntry, DataMatches, FoldedFile};
pub use outside::OutsideOutline;

// The longest qualified name, in bytes, that `short_ascii_holds` lowers on the stack.
const SHORT_QUALNAME: usize = 128;

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
    /// One card per code match the limit kept, and how many code symbols matched.
    pub cards: Bounded<SymbolCard>,
    /// The matching keys of data files, which rank after every code match.
    pub data: DataMatches,
    /// The data keys that match and that the kind filter leaves out.
    pub data_kind_excluded: usize,
    /// With no code card and no kind filter, where data files spell the query outside their
    /// outline.
    pub outside: OutsideOutline,
    /// The files the scan passed over.
    pub skipped: Vec<Skipped>,
    /// The kind filter's name, if any.
    pub kind: Option<&'static str>,
    /// On a miss that the kind filter caused, the kinds of the symbols the query matches.
    pub other_kinds: Vec<&'static str>,
    /// On a miss, each place the query is spelled where a file did not parse.
    pub unparsed: Vec<(String, u32)>,
}

impl FindResult {
    /// Returns whether nothing matched: no code card, no data entry and no unlisted data file.
    #[must_use]
    pub fn is_miss(&self) -> bool {
        self.cards.rows().is_empty()
            && self.data.entries.is_empty()
            && self.data.unlisted.is_empty()
    }
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
    let value = loaded.value.and_then(|scan| {
        let answered = answered_over(scan, |scan| answer(reading, scan, request, limit));
        diagnostics.extend(answered.diagnostics);
        answered.value
    });
    Noted { value, diagnostics }
}

// The matches. With no code card and no kind filter, where data files spell the query outside
// their outline. On a miss, each place where a file that did not parse spells the query.
fn answer(
    reading: &Reading<'_>,
    scan: &SymbolFacts,
    request: &FindRequest,
    limit: usize,
) -> Noted<Result<FindResult, Error>> {
    let found = matched(scan, request, limit);
    let outside = if request.kind.is_none() && found.cards.rows().is_empty() {
        outside_outline(reading, scan, &lower(&request.query))
    } else {
        Noted::new(Ok(OutsideOutline::default()))
    };
    outside.then_ok(|outside| {
        let found = FindResult { outside, ..found };
        if !found.is_miss() {
            return Noted::new(Ok(found));
        }
        let unparsed = unparsed_mentions(reading, scan, &request.query);
        unparsed.map_ok(|unparsed| FindResult { unparsed, ..found })
    })
}

// The result without the places outside the outlines and the unparsed places. A data key has no
// kind, so a kind filter leaves every data key out and counts the ones that match.
fn matched(scan: &SymbolFacts, request: &FindRequest, limit: usize) -> FindResult {
    let needle = lower(&request.query);
    let symbols = scan.files.iter().flat_map(|file| {
        let symbols = file.symbols.iter();
        symbols.map(move |symbol| (file, symbol))
    });
    let (data, code): (Vec<_>, Vec<_>) = symbols
        .filter(|(file, symbol)| {
            let data = is_data_language(file.language);
            matches(symbol, &needle, if data { None } else { request.kind })
        })
        .partition(|(file, _)| is_data_language(file.language));
    let (data, data_kind_excluded) = match request.kind {
        Some(_) => (Vec::new(), data.len()),
        None => (data, 0),
    };
    let code_total = code.len();
    let other_kinds = match request.kind {
        Some(_) if code_total == 0 => other_kinds(scan, &needle),
        _ => Vec::new(),
    };
    let cards = top_cards(code, &needle, limit);
    let room = DATA_ENTRIES.min(limit.saturating_sub(cards.len()));
    FindResult {
        query: request.query.clone(),
        cards: Bounded::with_total(cards, code_total, limit),
        data: data_matches(data, &needle, room),
        data_kind_excluded,
        outside: OutsideOutline::default(),
        skipped: scan.skipped.clone(),
        kind: request.kind.map(SymbolKind::as_str),
        other_kinds,
        unparsed: Vec::new(),
    }
}

// The cards of the first `limit` matches by rank, lowered name and path. The scan index breaks
// the last ties, as a stable sort of the scan order does, so only the kept matches need a card.
fn top_cards(matches: Vec<(&FileSymbols, &Symbol)>, needle: &str, limit: usize) -> Vec<SymbolCard> {
    let mut keyed: Vec<_> = (matches.into_iter().enumerate())
        .map(|(at, (file, symbol))| {
            let (rank, lowered) = match_rank(symbol, needle);
            ((rank, lowered, file.path.as_str(), at), file, symbol)
        })
        .collect();
    if keyed.len() > limit {
        keyed.select_nth_unstable_by(limit, |left, right| left.0.cmp(&right.0));
        keyed.truncate(limit);
    }
    keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    (keyed.iter())
        .map(|(_, file, symbol)| symbol_card(file.language, &file.path, symbol, String::new()))
        .collect()
}

// The kinds, sorted, of the code symbols that the query matches with no kind filter.
fn other_kinds(scan: &SymbolFacts, needle: &str) -> Vec<&'static str> {
    let code = (scan.files.iter()).filter(|file| !is_data_language(file.language));
    let symbols = code.flat_map(|file| file.symbols.iter());
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
    short_ascii_holds(symbol, needle).unwrap_or_else(|| {
        lower(&symbol.name).contains(needle) || lower(&symbol.qualname()).contains(needle)
    })
}

// Whether the lowered qualified name holds `needle`, or `None` for a name that is not ASCII or is
// long. ASCII lowers byte by byte and the name ends the qualified name, so one test covers both.
fn short_ascii_holds(symbol: &Symbol, needle: &str) -> Option<bool> {
    let (parent, name) = (symbol.parent.as_bytes(), symbol.name.as_bytes());
    let dot: &[u8] = if parent.is_empty() { b"" } else { b"." };
    let length = parent.len() + dot.len() + name.len();
    if length > SHORT_QUALNAME || !parent.is_ascii() || !name.is_ascii() {
        return None;
    }
    let mut buffer = [0_u8; SHORT_QUALNAME];
    let joined = &mut buffer[..length];
    let (head, tail) = joined.split_at_mut(parent.len());
    let (middle, last) = tail.split_at_mut(dot.len());
    head.copy_from_slice(parent);
    middle.copy_from_slice(dot);
    last.copy_from_slice(name);
    joined.make_ascii_lowercase();
    std::str::from_utf8(joined)
        .ok()
        .map(|joined| joined.contains(needle))
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
    for file in &scan.files {
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

// The lines of one file where the query is spelled inside a region that did not parse. The
// call's deadline is checked first, for every scanned file, as the scan checks it.
fn mentions_in(
    reading: &Reading<'_>,
    file: &FileSymbols,
    needle: &str,
    query: &str,
) -> Result<Noted<Vec<u32>>, Error> {
    checkpoint(reading.deadline)?;
    if needle.is_empty() || !joins_name_graph(file.language) {
        return Ok(Noted::new(Vec::new()));
    }
    let absolute = io::contained_path(&reading.repo.root, Path::new(&file.path))
        .map_err(|refusal| kernel::Error::SecurityRefusal(refusal.to_string()))?;
    let Ok(bytes) = io::read_bounded(&absolute) else {
        return Ok(Noted::new(Vec::new()));
    };
    let text = FileText::decode(file.path.clone(), file.language, &bytes);
    if !lower(&text.text).contains(needle) {
        return Ok(Noted::new(Vec::new()));
    }
    let Served { value, warning } = reading.facts.facts(&text, reading.deadline);
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
