//! The symbol views' two forms: the expansion and the lookup as cards in the text, and each
//! card's fields in the JSON.

use app::{ExpandResult, FindResult, SymbolCard, cards_text, omitted_line};
use prompts::messages::{
    EXPAND_BATCH_SHORTENED, FIND_NO_MATCHES, FIND_NO_MATCHES_KIND, FIND_UNPARSED,
};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json, skipped_line};
use crate::{Rendered, View};

// A missed query names at most this many unparsed places before it counts the rest.
const UNPARSED_SHOWN: usize = 5;

impl View for ExpandResult {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(expansion_text(self), ExpandJson::of(self), None, wrap_json)
    }
}

impl View for FindResult {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(find_text(self), FindJson::of(self), None, wrap_json)
    }
}

// Any skipped-file warning first, then the cards, or why nothing matched and where a lost
// declaration may sit.
fn find_text(result: &FindResult) -> String {
    let cards = &result.cards;
    let mut body = if cards.rows().is_empty() {
        miss_line(result) + "\n"
    } else {
        cards_text(cards.rows())
    };
    if cards.omitted() > 0 {
        body.push_str(&omitted_line(cards.omitted(), "matches", cards.limit()));
        body.push('\n');
    }
    if cards.rows().is_empty() && !result.unparsed.is_empty() {
        body.push_str(&unparsed_line(result));
        body.push('\n');
    }
    let warning = skipped_line(&result.skipped);
    if warning.is_empty() {
        body
    } else {
        format!("{warning}\n\n{body}")
    }
}

// A miss that the kind filter caused names the kinds the query does match.
fn miss_line(result: &FindResult) -> String {
    match result.kind {
        Some(kind) if !result.other_kinds.is_empty() => {
            let kinds = result.other_kinds.join(", ");
            FIND_NO_MATCHES_KIND.render((kind, &result.query, &kinds))
        }
        _ => FIND_NO_MATCHES.render(&result.query),
    }
}

fn unparsed_line(result: &FindResult) -> String {
    let shown: Vec<String> = result
        .unparsed
        .iter()
        .take(UNPARSED_SHOWN)
        .map(|(path, line)| format!("{}:{line}", one_line(path)))
        .collect();
    let hidden = result.unparsed.len().saturating_sub(UNPARSED_SHOWN);
    let listed = if hidden > 0 {
        format!("{}; ... {hidden} more", shown.join(", "))
    } else {
        shown.join(", ")
    };
    FIND_UNPARSED.render((&one_line(&result.query), &listed))
}

#[derive(Clone, Serialize)]
struct FindJson<'a> {
    query: &'a str,
    kind: Option<&'a str>,
    other_kinds: &'a [&'static str],
    total: usize,
    limit: usize,
    omitted: usize,
    matches: Vec<CardJson<'a>>,
    skipped: Vec<SkippedJson<'a>>,
    unparsed: Vec<PlaceJson<'a>>,
}

#[derive(Clone, Serialize)]
struct PlaceJson<'a> {
    path: &'a str,
    line: u32,
}

impl<'a> FindJson<'a> {
    fn of(result: &'a FindResult) -> Self {
        let cards = &result.cards;
        let unparsed = result
            .unparsed
            .iter()
            .map(|(path, line)| PlaceJson { path, line: *line });
        FindJson {
            query: &result.query,
            kind: result.kind,
            other_kinds: &result.other_kinds,
            total: cards.total(),
            limit: cards.limit(),
            omitted: cards.omitted(),
            matches: cards.rows().iter().map(CardJson::of).collect(),
            skipped: skipped_json(&result.skipped),
            unparsed: unparsed.collect(),
        }
    }
}

impl Listing for FindJson<'_> {
    fn items(&self) -> usize {
        self.matches.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.matches.truncate(kept);
        prefix
    }
}

/// Returns one stderr row for each id of `result` that did not resolve, and a count of the misses
/// that the answer does not name.
#[must_use]
pub fn expand_stderr_rows(result: &ExpandResult) -> Vec<String> {
    let mut rows: Vec<String> = result
        .unresolved
        .iter()
        .map(|(entry, reason)| format!("unresolved: {entry} -- {reason}"))
        .collect();
    if result.unresolved_omitted > 0 {
        rows.push(format!(
            "unresolved: {} further ids are not listed; {} of the ids requested did not resolve",
            result.unresolved_omitted,
            result.unresolved_total()
        ));
    }
    rows
}

// The cards, then the note that names how many bodies the budget cut. The misses are not here:
// each door reports them on its own channel.
pub(crate) fn expansion_text(result: &ExpandResult) -> String {
    let cards = cards_text(&result.cards);
    let shortened = result.shortened();
    if shortened == 0 {
        return cards;
    }
    let note = EXPAND_BATCH_SHORTENED.render((shortened, result.cards.len(), result.budget));
    format!("{cards}\n{note}")
}

// One card's fields: the body only when there is one, the cut only when the card shows fewer
// lines than the symbol has.
#[derive(Clone, Serialize)]
pub(crate) struct CardJson<'a> {
    stable_id: &'a str,
    path: &'a str,
    start_line: u32,
    end_line: u32,
    kind: &'a str,
    language: &'a str,
    signature: &'a str,
    parent_class: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    body: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_truncated: Option<BodyCut>,
}

#[derive(Clone, Copy, Serialize)]
struct BodyCut {
    lines_shown: usize,
    lines: usize,
}

impl<'a> CardJson<'a> {
    pub(crate) fn of(card: &'a SymbolCard) -> Self {
        let cut = card.body_shown < card.body_total;
        CardJson {
            stable_id: &card.stable_id,
            path: &card.path,
            start_line: card.start_line,
            end_line: card.end_line,
            kind: card.kind,
            language: card.language,
            signature: &card.signature,
            parent_class: &card.parent_class,
            body: &card.body,
            body_truncated: cut.then_some(BodyCut {
                lines_shown: card.body_shown,
                lines: card.body_total,
            }),
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct ExpandJson<'a> {
    budget_tokens: usize,
    shortened: usize,
    pub(crate) symbols: Vec<CardJson<'a>>,
    unresolved: Vec<UnresolvedJson<'a>>,
    unresolved_total: usize,
    unresolved_omitted: usize,
}

#[derive(Clone, Serialize)]
struct UnresolvedJson<'a> {
    stable_id: &'a str,
    reason: &'a str,
}

impl<'a> ExpandJson<'a> {
    pub(crate) fn of(result: &'a ExpandResult) -> Self {
        let unresolved = result
            .unresolved
            .iter()
            .map(|(entry, reason)| UnresolvedJson {
                stable_id: entry,
                reason,
            });
        ExpandJson {
            budget_tokens: result.budget,
            shortened: result.shortened(),
            symbols: result.cards.iter().map(CardJson::of).collect(),
            unresolved: unresolved.collect(),
            unresolved_total: result.unresolved_total(),
            unresolved_omitted: result.unresolved_omitted,
        }
    }
}

impl Listing for ExpandJson<'_> {
    fn items(&self) -> usize {
        self.symbols.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.symbols.truncate(kept);
        prefix
    }
}
