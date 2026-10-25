//! The symbol views' two forms: the expansion and the lookup as cards in the text, and each
//! card's fields in the JSON.

use app::{DataEntry, DataMatches, ExpandResult, FindResult, SymbolCard, cards_text, omitted_line};
use prompts::messages::{
    EXPAND_BATCH_SHORTENED, FIND_DATA_FOLDED, FIND_DATA_NOT_SYMBOLS, FIND_DATA_UNLISTED,
    FIND_NO_MATCHES, FIND_NO_MATCHES_KIND, FIND_UNPARSED,
};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json, skipped_line};
use crate::{Rendered, View};

// A missed query names at most this many unparsed places before it counts the rest.
const UNPARSED_SHOWN: usize = 5;
// The line for the data files no entry shows names at most this many of them.
const UNLISTED_NAMED: usize = 3;

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

// Any skipped-file warning first, then the code cards and the data entries, or why nothing
// matched and where a lost declaration may sit.
fn find_text(result: &FindResult) -> String {
    let cards = &result.cards;
    let mut body = if result.is_miss() {
        miss_text(result)
    } else {
        matches_text(result)
    };
    if cards.omitted() > 0 {
        body.push_str(&omitted_line(cards.omitted(), "matches", cards.limit()));
        body.push('\n');
    }
    if let Some(line) = unlisted_line(&result.data) {
        body.push_str(&line);
        body.push('\n');
    }
    if result.is_miss() && !result.unparsed.is_empty() {
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

// The code cards, then each data entry: a key's card or one line for a folded file.
fn matches_text(result: &FindResult) -> String {
    let code = result.cards.rows().iter().map(card_block);
    let data = result.data.entries.iter().map(|entry| match entry {
        DataEntry::Card(card) => card_block(card),
        DataEntry::Folded(file) => FIND_DATA_FOLDED.render((&one_line(&file.path), file.keys)),
    });
    let blocks: Vec<String> = code.chain(data).collect();
    blocks.join("\n\n") + "\n"
}

fn card_block(card: &SymbolCard) -> String {
    cards_text(std::slice::from_ref(card)).trim_end().to_owned()
}

// A miss in a repository with data files says that deep keys are not symbols.
fn miss_text(result: &FindResult) -> String {
    let mut text = miss_line(result) + "\n";
    if result.data.scanned {
        text.push_str(FIND_DATA_NOT_SYMBOLS);
        text.push('\n');
    }
    text
}

// The data files no entry shows, with the first few named.
fn unlisted_line(data: &DataMatches) -> Option<String> {
    if data.unlisted.is_empty() {
        return None;
    }
    let named = data.unlisted.iter().take(UNLISTED_NAMED);
    let mut listed: Vec<String> = named
        .map(|file| format!("{} ({})", one_line(&file.path), file.keys))
        .collect();
    if data.unlisted.len() > UNLISTED_NAMED {
        listed.push("...".to_owned());
    }
    let keys = data.unlisted.iter().map(|file| file.keys).sum::<usize>();
    let listed = listed.join(", ");
    Some(FIND_DATA_UNLISTED.render((keys, data.unlisted.len(), listed.as_str())))
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
    folded: Vec<FoldedJson<'a>>,
    skipped: Vec<SkippedJson<'a>>,
    unparsed: Vec<PlaceJson<'a>>,
}

#[derive(Clone, Serialize)]
struct PlaceJson<'a> {
    path: &'a str,
    line: u32,
}

// A data file whose matching keys have no card: a folded entry or an unlisted file.
#[derive(Clone, Serialize)]
struct FoldedJson<'a> {
    path: &'a str,
    keys: usize,
}

impl<'a> FindJson<'a> {
    fn of(result: &'a FindResult) -> Self {
        let cards = &result.cards;
        let unparsed = result
            .unparsed
            .iter()
            .map(|(path, line)| PlaceJson { path, line: *line });
        let mut matches: Vec<CardJson<'a>> = cards.rows().iter().map(CardJson::of).collect();
        let mut folded = Vec::new();
        for entry in &result.data.entries {
            match entry {
                DataEntry::Card(card) => matches.push(CardJson::of(card)),
                DataEntry::Folded(file) => folded.push(FoldedJson::of(file)),
            }
        }
        folded.extend(result.data.unlisted.iter().map(FoldedJson::of));
        let total = cards.total() + result.data.total();
        FindJson {
            query: &result.query,
            kind: result.kind,
            other_kinds: &result.other_kinds,
            total,
            limit: cards.limit(),
            omitted: total - matches.len(),
            matches,
            folded,
            skipped: skipped_json(&result.skipped),
            unparsed: unparsed.collect(),
        }
    }
}

impl<'a> FoldedJson<'a> {
    fn of(file: &'a app::FoldedFile) -> Self {
        FoldedJson {
            path: &file.path,
            keys: file.keys,
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

/// Returns one stderr row per unresolved id of `result`, then the count of unnamed misses.
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

#[cfg(test)]
mod tests;
