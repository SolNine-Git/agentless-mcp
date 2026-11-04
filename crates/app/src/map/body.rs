//! The body map: the function map's file rows, then the bodies of its best symbols, cut to the
//! map's own budget, in one answer.

use io::Granularity;

use super::{MapRequest, MapResult, map, map_rows_text};
use crate::expand::{EXPAND_MAX_SEATS, ExpandResult, expand_one, first_copies, fitted};
use crate::span::ParsedFiles;
use crate::{Error, MapFile, Noted, Reading, Services, SymbolCard};

// The tokens one body seat stands for: the map's budget over this is the seat count.
const BODY_TOKENS_PER_SEAT: usize = 300;

/// The map's file rows without their symbol rows, and the bodies of the map's best symbols.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyMapResult {
    /// The map, each file stripped of its symbol rows, with the token count of what remains. A
    /// file's count leaves out the symbols that have a body here.
    pub map: MapResult,
    /// The bodies, focus symbols first, then by the packing's score. No two of them share a
    /// line of one file.
    pub bodies: ExpandResult,
}

/// Builds the function map that `request` asks for, then expands its top symbols. The scan's
/// warnings come with either outcome.
#[must_use]
pub fn body_map(
    services: &Services,
    reading: &Reading<'_>,
    request: &MapRequest,
) -> Noted<Result<BodyMapResult, Error>> {
    let function = MapRequest {
        granularity: Some(Granularity::Function),
        ..request.clone()
    };
    let built = map(services, reading, &function);
    let mut diagnostics = built.diagnostics;
    let base = match built.value {
        Ok(base) => base,
        Err(error) => {
            return Noted {
                value: Err(error),
                diagnostics,
            };
        }
    };
    let order: Vec<String> = base
        .focus_order
        .iter()
        .chain(&base.expand_order)
        .cloned()
        .collect();
    let seats = (base.budget / BODY_TOKENS_PER_SEAT).clamp(1, EXPAND_MAX_SEATS);
    let seated = seat_apart(reading, &first_copies(&order), seats);
    diagnostics.extend(seated.diagnostics);
    let Seated { cards, unresolved } = seated.value;
    let files: Vec<MapFile> = (base.files.into_iter())
        .map(|file| without_rows(file, &cards))
        .collect();
    let rendered = services.counter.count(&map_rows_text(&files));
    let bodies = fitted(&*services.counter, cards, unresolved, base.budget);
    let map = MapResult {
        files,
        rendered,
        ..base
    };
    Noted {
        value: Ok(BodyMapResult { map, bodies }),
        diagnostics,
    }
}

// The bodies seated so far, and each id that resolved to none with the reason.
#[derive(Default)]
struct Seated {
    cards: Vec<SymbolCard>,
    unresolved: Vec<(String, String)>,
}

// The ids in map order until the seats are full. An id whose lines meet a seated body of its file
// gives up its seat to the next id, so no source line prints twice.
fn seat_apart(reading: &Reading<'_>, ids: &[&str], seats: usize) -> Noted<Seated> {
    let mut noted = Noted::new(Seated::default());
    let mut files = ParsedFiles::new();
    for raw in ids {
        if noted.value.cards.len() == seats {
            break;
        }
        let one = expand_one(reading, raw, &mut files);
        noted.diagnostics.extend(one.diagnostics);
        match one.value {
            Ok(card) if meets_seated(&noted.value.cards, &card) => {}
            Ok(card) => noted.value.cards.push(card),
            Err(reason) => noted.value.unresolved.push(((*raw).to_owned(), reason)),
        }
    }
    noted
}

fn meets_seated(seated: &[SymbolCard], card: &SymbolCard) -> bool {
    seated.iter().any(|held| {
        held.path == card.path
            && held.start_line <= card.end_line
            && card.start_line <= held.end_line
    })
}

// The file's symbol rows go, and its count leaves out each symbol that a body shows.
fn without_rows(file: MapFile, cards: &[SymbolCard]) -> MapFile {
    let shown = cards.iter().filter(|card| card.path == file.path).count();
    MapFile {
        entries: Vec::new(),
        total: file.total.saturating_sub(shown),
        ..file
    }
}

#[cfg(test)]
mod tests;
