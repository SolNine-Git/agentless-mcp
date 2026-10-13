//! Line-numbered bodies for named stable ids, cut max-min fair to one token budget, with every id
//! that did not resolve named or counted.

use std::collections::HashSet;
use std::slice;

use facts::line_prefix;
use prompts::messages::{EXPAND_BODY_TRUNCATED, EXPAND_NO_ROOM, GROUPED_IDS};

use crate::bounds::within;
use crate::budget::{TRUNCATION_MARKER_TOKENS, allocate, longest_fitting};
use crate::ops::limits::MAX_LIMIT;
use crate::span::{Missed, SymbolSpan, resolve_symbol_span};
use crate::{
    Error, IntArg, Noted, Reading, Services, SymbolCard, TokenCounter, cards_text, symbol_card,
};

// What one expansion's cards may cost: under the output ceiling by the receipt's margin.
pub(crate) const EXPAND_BUDGET_TOKENS: usize = 12_000;
// How many cards one call seats, however small each is cut: the headers count against the ceiling.
pub(crate) const EXPAND_MAX_SEATS: usize = 40;
// How many missed ids the answer names before it counts the rest.
const MAX_UNRESOLVED_ROWS: usize = 20;

/// An expand request: the stable ids, and the most ids the call expands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandRequest {
    /// The stable ids, in the caller's order. A repeated id expands once.
    pub ids: Vec<String>,
    /// The most ids the call expands.
    pub limit: IntArg,
}

/// The cards of the ids that resolved, fitted to the budget, and the ids that did not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandResult {
    /// One card per resolved id, with its body, in request order.
    pub cards: Vec<SymbolCard>,
    /// Each named miss: the id, or a count of ids that share the reason, and the reason.
    pub unresolved: Vec<(String, String)>,
    /// The token budget the bodies were cut to.
    pub budget: usize,
    /// The misses that the row bound left unnamed.
    pub unresolved_omitted: usize,
}

impl ExpandResult {
    /// Returns how many cards show fewer body lines than their symbol has.
    #[must_use]
    pub fn shortened(&self) -> usize {
        let cut = self
            .cards
            .iter()
            .filter(|card| card.body_shown < card.body_total);
        cut.count()
    }

    /// Returns how many misses there are, named or not.
    #[must_use]
    pub fn unresolved_total(&self) -> usize {
        self.unresolved.len() + self.unresolved_omitted
    }
}

// The bounds of one expansion: the ids it expands, the budget its bodies share, and its cards.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seating {
    pub(crate) limit: usize,
    pub(crate) budget: usize,
    pub(crate) seats: usize,
}

/// Expands the stable ids of `request` in the repository that `reading` names. Each cache row
/// warning comes with either outcome.
#[must_use]
pub fn expand(
    services: &Services,
    reading: &Reading<'_>,
    request: &ExpandRequest,
) -> Noted<Result<ExpandResult, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    let seating = Seating {
        limit,
        budget: EXPAND_BUDGET_TOKENS,
        seats: EXPAND_MAX_SEATS,
    };
    expand_symbols(services, reading, &request.ids, seating).map(Ok)
}

pub(crate) fn expand_symbols(
    services: &Services,
    reading: &Reading<'_>,
    ids: &[String],
    seating: Seating,
) -> Noted<ExpandResult> {
    let requested = first_copies(ids);
    let mut noted = Noted::new(Vec::new());
    let mut unresolved = Vec::new();
    for raw in requested.iter().take(seating.limit) {
        let one = expand_one(reading, raw);
        noted.diagnostics.extend(one.diagnostics);
        match one.value {
            Ok(card) => noted.value.push(card),
            Err(reason) => unresolved.push(((*raw).to_owned(), reason)),
        }
    }
    let over_limit = requested.len().saturating_sub(seating.limit);
    unresolved.extend(grouped_rows(
        over_limit,
        ids.len() - requested.len(),
        seating.limit,
    ));
    let mut cards = noted.value;
    if cards.len() > seating.seats {
        let reason = EXPAND_NO_ROOM.render((cards.len(), seating.seats));
        let count = GROUPED_IDS.render(cards.len() - seating.seats);
        unresolved.push((count, reason));
        cards.truncate(seating.seats);
    }
    let unresolved_omitted = unresolved.len().saturating_sub(MAX_UNRESOLVED_ROWS);
    unresolved.truncate(MAX_UNRESOLVED_ROWS);
    let result = ExpandResult {
        cards: fit_bodies(&*services.counter, cards, seating.budget),
        unresolved,
        budget: seating.budget,
        unresolved_omitted,
    };
    Noted {
        value: result,
        diagnostics: noted.diagnostics,
    }
}

// Each id once, at its first position.
pub(crate) fn first_copies(ids: &[String]) -> Vec<&str> {
    let mut seen = HashSet::new();
    let firsts = ids.iter().filter(|id| seen.insert(id.as_str()));
    firsts.map(String::as_str).collect()
}

// One row for the ids past the limit, then one for the repeated ids.
fn grouped_rows(over_limit: usize, repeated: usize, limit: usize) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if over_limit > 0 {
        let reason = format!("not expanded: the per-call limit is {limit} symbols");
        rows.push((GROUPED_IDS.render(over_limit), reason));
    }
    if repeated > 0 {
        let reason = "not expanded: repeated in this batch, and expanded under the first copy";
        rows.push((GROUPED_IDS.render(repeated), reason.to_owned()));
    }
    rows
}

// A path outside the root is one more reason, beside the ids that do not resolve.
fn expand_one(reading: &Reading<'_>, raw: &str) -> Noted<Result<SymbolCard, String>> {
    resolve_symbol_span(reading, raw).map(|span| {
        span.map(|span| body_card(&span))
            .map_err(|missed| match missed {
                Missed::Reason(reason) => reason,
                Missed::Refused(refusal) => refusal.to_string(),
            })
    })
}

// The card with the span's lines, each behind its line number.
fn body_card(span: &SymbolSpan) -> SymbolCard {
    let first = (span.start_line as usize).saturating_sub(1);
    let lines = span.source.split('\n').skip(first);
    let numbered: Vec<String> = (span.start_line..=span.end_line)
        .zip(lines)
        .map(|(number, line)| format!("{}{line}", line_prefix(number)))
        .collect();
    symbol_card(span.language, &span.path, &span.symbol, numbered.join("\n"))
}

// Cards that fit an equal share stay whole; the others are cut to the last share.
fn fit_bodies(
    counter: &dyn TokenCounter,
    cards: Vec<SymbolCard>,
    budget: usize,
) -> Vec<SymbolCard> {
    let costs: Vec<usize> = cards
        .iter()
        .map(|card| counter.count(&cards_text(slice::from_ref(card))))
        .collect();
    let (cut, share) = allocate(&costs, budget);
    cards
        .into_iter()
        .enumerate()
        .map(|(index, card)| {
            if cut.contains(&index) {
                shorten(counter, &card, share)
            } else {
                card
            }
        })
        .collect()
}

// At least one body line survives any share, so the card still points somewhere.
fn shorten(counter: &dyn TokenCounter, card: &SymbolCard, share: usize) -> SymbolCard {
    let lines: Vec<&str> = card.body.split('\n').collect();
    let bare = SymbolCard {
        body: String::new(),
        ..card.clone()
    };
    let header = counter.count(&cards_text(slice::from_ref(&bare)));
    let room = share.checked_sub(header + TRUNCATION_MARKER_TOKENS);
    let fits =
        |kept: usize| room.is_some_and(|room| counter.count(&lines[..kept].join("\n")) <= room);
    let kept = longest_fitting(1, lines.len(), fits);
    let marker = EXPAND_BODY_TRUNCATED.render((kept, lines.len()));
    SymbolCard {
        body: format!("{}\n{marker}", lines[..kept].join("\n")),
        body_shown: kept,
        body_total: lines.len(),
        ..card.clone()
    }
}

#[cfg(test)]
mod tests;
