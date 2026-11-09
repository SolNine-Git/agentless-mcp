//! The trace answer's two forms: one card per target in the text, as the trace grammar words it,
//! and each card's typed values in the JSON, trimmed at the cards.

mod diff;
mod file;
mod json;
mod miss;
mod range;
mod symbol;

use app::trace::{Card, Outcome, Revision, Section, Tally, Trace, TraceRequest, Traced};
use app::{ROW_INDENT, SymbolCard};
use prompts::messages::{
    TRACE_AT_LEAST, TRACE_MORE_HINT, TRACE_MORE_HINT_REV, TRACE_ROWS_UNLISTED,
    TRACE_ROWS_UNLISTED_FLOOR, TRACE_SAME_CARD, TRACE_SECTION_NONE, TRACE_TARGET,
};
use text::one_line;

use crate::envelope::wrap_json;
use crate::{Rendered, View};

pub(crate) use file::listing_row;
pub(crate) use json::{PlaceJson, RowJson, places};

// A row line of a list section holds at most this many items.
const ITEMS_PER_LINE: usize = 8;

/// A trace answer: the request, whose `more` names the section each card shows in full, and the
/// cards that answer it.
#[derive(Clone, Copy, Debug)]
pub struct TraceAnswer<'a> {
    /// The request.
    pub request: &'a TraceRequest,
    /// The cards, one per target.
    pub trace: &'a Trace,
}

impl View for TraceAnswer<'_> {
    fn rendered(&self) -> Rendered<'_> {
        let payload = json::TraceJson::of(self.request, self.trace);
        Rendered::new(trace_text(self), payload, None, wrap_json)
    }
}

/// Returns the cards of `answer` with no receipt: one block per target in request order, each
/// headed by its target when the request names more than one.
#[must_use]
pub fn trace_text(answer: &TraceAnswer<'_>) -> String {
    let cards = &answer.trace.cards;
    let more = More {
        section: answer.request.more,
        revision: answer.trace.revision.as_ref(),
    };
    let blocks: Vec<String> = (cards.iter())
        .map(|traced| {
            let lines = outcome_lines(traced, more);
            if cards.len() > 1 {
                let target = TRACE_TARGET.render(&one_line(&traced.target));
                format!("{target}\n{}", lines.join("\n"))
            } else {
                lines.join("\n")
            }
        })
        .collect();
    blocks.join("\n\n") + "\n"
}

// The section that `more` names, and the revision that the answer read, which each hint repeats
// so that the next call reads the same commit.
#[derive(Clone, Copy, Debug)]
struct More<'a> {
    section: Option<Section>,
    revision: Option<&'a Revision>,
}

impl More<'_> {
    fn is_none(self) -> bool {
        self.section.is_none()
    }

    fn is_some(self) -> bool {
        self.section.is_some()
    }

    // The hint at the call that shows `section` in full.
    fn hint(self, section: Section) -> String {
        match self.revision {
            None => TRACE_MORE_HINT.render(section.as_str()),
            Some(revision) => {
                TRACE_MORE_HINT_REV.render((&one_line(&revision.name), section.as_str()))
            }
        }
    }
}

fn outcome_lines(traced: &Traced, more: More<'_>) -> Vec<String> {
    match &traced.outcome {
        Outcome::Same(first) => vec![TRACE_SAME_CARD.render(first + 1)],
        Outcome::NoSection { section, kind } => vec![miss::no_section(*section, *kind)],
        Outcome::Card(card) => card_lines(card, &traced.target, more),
    }
}

fn card_lines(card: &Card, target: &str, more: More<'_>) -> Vec<String> {
    match card {
        Card::Symbol(traced) => symbol::symbol_lines(traced, more),
        Card::File(traced) => file::file_lines(traced, more),
        Card::Folder(traced) => file::folder_lines(traced, more),
        Card::Candidates(traced) => miss::candidates_lines(traced, target, more),
        Card::Miss(missed) => miss::miss_lines(missed, target, more.revision),
        Card::Range(traced) => range::range_lines(traced),
        Card::Diff(traced) => diff::diff_lines(traced, more),
    }
}

// One definition as a candidates card or a range card lists it: the symbol card's header.
fn card_row(card: &SymbolCard) -> String {
    format!("{ROW_INDENT}{}", symbol::header(card))
}

// The head of a section: its name, then its parts, then the hint at `more` when the default card
// hides some of the section's rows.
fn head(section: Section, parts: &[String], more: More<'_>, hidden: bool) -> String {
    let mut line = format!("{}: {}", section.as_str(), parts.join("; "));
    if more.is_none() && hidden {
        line.push_str("  ");
        line.push_str(&more.hint(section));
    }
    line
}

fn none(section: Section) -> String {
    TRACE_SECTION_NONE.render(section.as_str())
}

// A count as the card prints it: the number, or its floor when the counted list was cut first.
fn tally(count: Tally) -> String {
    match count {
        Tally::Exact(count) => count.to_string(),
        Tally::Floor(count) => TRACE_AT_LEAST.render(count),
    }
}

fn is_cut(shown: usize, total: Tally) -> bool {
    match total {
        Tally::Exact(count) | Tally::Floor(count) => count > shown,
    }
}

// The line under rows that a cap cut, which counts the rows it left out.
fn unlisted(shown: usize, total: Tally) -> Option<String> {
    let line = match total {
        Tally::Exact(count) if count > shown => TRACE_ROWS_UNLISTED.render(count - shown),
        Tally::Floor(count) if count > shown => TRACE_ROWS_UNLISTED_FLOOR.render(count - shown),
        Tally::Exact(_) | Tally::Floor(_) => return None,
    };
    Some(format!("{ROW_INDENT}{line}"))
}

// The items of a list section, joined on indented row lines.
fn item_lines(items: &[String]) -> Vec<String> {
    (items.chunks(ITEMS_PER_LINE))
        .map(|chunk| format!("{ROW_INDENT}{}", chunk.join(", ")))
        .collect()
}

pub(crate) fn span(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

// A tier's wire value as a row prints it, such as `same-file`.
fn tier_word(tier: &str) -> String {
    one_line(tier).replace('_', "-")
}
