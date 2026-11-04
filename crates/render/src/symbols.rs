//! The symbol views' two forms: the expansion and the lookup as cards in the text, and each
//! card's fields in the JSON.

use app::{
    DataEntry, DataMatches, ExpandResult, FindResult, OutsideOutline, SymbolCard, Usage, UsageFile,
    UsageOrder, cards_text,
};
use prompts::messages::{
    EXPAND_BATCH_SHORTENED, EXPAND_UNRESOLVED_UNLISTED, FIND_DATA_FOLDED, FIND_DATA_FOLDED_REST,
    FIND_DATA_KIND_EXCLUDED, FIND_DATA_OUTSIDE, FIND_DATA_OUTSIDE_UNREAD, FIND_DATA_UNLISTED,
    FIND_NO_DECLARATION, FIND_NO_MATCHES, FIND_NO_MATCHES_KIND, FIND_UNLISTED, FIND_UNPARSED,
    FIND_USAGE_HEAD, FIND_USAGE_HEAD_OWNER, FIND_USAGE_LINES, FIND_USAGE_LINES_ALL,
    FIND_USAGE_NEAREST, FIND_USAGE_NEAREST_ALL, FIND_USAGE_REST, FIND_USAGE_REST_OWNER,
    FIND_USAGE_ROW,
};
use prompts::{Count, Template, Text};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json, skipped_line};
use crate::{Rendered, View};

// A missed query names at most this many unparsed places before it counts the rest.
const UNPARSED_SHOWN: usize = 5;
// The line for the data files no entry shows names at most this many of them.
const UNLISTED_NAMED: usize = 3;
// The line for the data files that spell the query outside their outline names at most this many.
const OUTSIDE_SHOWN: usize = 5;

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
// matched, then the notes on what the answer leaves out, then the usage files.
fn find_text(result: &FindResult) -> String {
    let mut body = if result.declares_nothing() {
        miss_line(result) + "\n"
    } else {
        matches_text(result)
    };
    for line in note_lines(result).into_iter().flatten() {
        body.push_str(&line);
        body.push('\n');
    }
    if let Some(usage) = &result.usage {
        body.push('\n');
        body.push_str(&usage_text(usage));
    }
    let warning = skipped_line(&result.skipped);
    if warning.is_empty() {
        body
    } else {
        format!("{warning}\n\n{body}")
    }
}

// The code matches past the limit, the unlisted data files, the data keys a kind filter left out,
// where a lost declaration may sit, and the data files that spell the query outside the outline.
fn note_lines(result: &FindResult) -> [Option<String>; 6] {
    let (cards, excluded) = (&result.cards, result.data_kind_excluded);
    [
        (cards.omitted() > 0).then(|| FIND_UNLISTED.render((cards.omitted(), cards.limit()))),
        unlisted_line(&result.data),
        (excluded > 0).then(|| FIND_DATA_KIND_EXCLUDED.render(excluded)),
        (result.declares_nothing() && !result.unparsed.is_empty()).then(|| unparsed_line(result)),
        outside_line(&result.query, &result.outside),
        (result.outside.unread > 0).then(|| {
            let outside = &result.outside;
            FIND_DATA_OUTSIDE_UNREAD.render((outside.unread, outside.budget))
        }),
    ]
}

// The code cards, then each data entry: a key's card or one line for a folded file.
fn matches_text(result: &FindResult) -> String {
    let code = result.cards.rows().iter().map(card_block);
    let data = result.data.entries.iter().map(|entry| match entry {
        DataEntry::Card(card) => card_block(card),
        DataEntry::Folded(file) => FIND_DATA_FOLDED.render((&one_line(&file.path), file.keys)),
        DataEntry::FoldedRest(file) => {
            FIND_DATA_FOLDED_REST.render((&one_line(&file.path), file.keys))
        }
    });
    let blocks: Vec<String> = code.chain(data).collect();
    blocks.join("\n\n") + "\n"
}

// The data files that spell the query outside their outline, the first few with their line.
fn outside_line(query: &str, outside: &OutsideOutline) -> Option<String> {
    let places = &outside.places;
    if places.is_empty() {
        return None;
    }
    let shown = places.iter().take(OUTSIDE_SHOWN);
    let mut listed: Vec<String> = shown
        .map(|(path, line)| format!("{} @{line}", one_line(path)))
        .collect();
    let hidden = places.len().saturating_sub(OUTSIDE_SHOWN);
    if hidden > 0 {
        listed.push(format!("... {hidden} more"));
    }
    let listed = listed.join(", ");
    Some(FIND_DATA_OUTSIDE.render((&one_line(query), places.len(), listed.as_str())))
}

fn card_block(card: &SymbolCard) -> String {
    cards_text(std::slice::from_ref(card)).trim_end().to_owned()
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
    let hidden = data.unlisted.len().saturating_sub(UNLISTED_NAMED);
    if hidden > 0 {
        listed.push(format!("... {hidden} more"));
    }
    let keys = data.unlisted.iter().map(|file| file.keys).sum::<usize>();
    let listed = listed.join(", ");
    let counts = (keys, data.unlisted.len(), data.room);
    Some(FIND_DATA_UNLISTED.render((counts.0, counts.1, counts.2, listed.as_str())))
}

// A miss that the kind filter caused names the kinds the query does match. With usage files to
// follow, the line does not send the reader to a text search.
fn miss_line(result: &FindResult) -> String {
    match result.kind {
        Some(kind) if !result.other_kinds.is_empty() => {
            let kinds = result.other_kinds.join(", ");
            FIND_NO_MATCHES_KIND.render((kind, &result.query, &kinds))
        }
        _ if result.usage.is_some() => FIND_NO_DECLARATION.render(&result.query),
        _ => FIND_NO_MATCHES.render(&result.query),
    }
}

// The head with its order, a row per listed file, and the count of the files the cap left out.
fn usage_text(usage: &Usage) -> String {
    let files = &usage.files;
    let mut lines = vec![usage_head(usage)];
    lines.extend(files.rows().iter().map(usage_row));
    if files.omitted() > 0 {
        let rest = (&FIND_USAGE_REST_OWNER, &FIND_USAGE_REST);
        lines.push(owner_form(usage, files.omitted(), rest));
    }
    lines.join("\n") + "\n"
}

// A row's form that names the owner of a qualified query, and its form with no owner.
type OwnerForms = (
    &'static Template<(Text, Text, Count)>,
    &'static Template<(Text, Count)>,
);

// The head and the count of the files left out both name the owner of a qualified query.
fn owner_form(usage: &Usage, count: usize, (with_owner, without): OwnerForms) -> String {
    let name = usage.name.as_str();
    match usage.owner.as_deref() {
        Some(owner) => with_owner.render((name, owner, count)),
        None => without.render((name, count)),
    }
}

// A head of one file ends with a colon and takes no order.
fn usage_head(usage: &Usage) -> String {
    let total = usage.files.total();
    let head = owner_form(usage, total, (&FIND_USAGE_HEAD_OWNER, &FIND_USAGE_HEAD));
    if total == 1 {
        return head;
    }
    let shown = usage.files.rows().len();
    let order = match (usage.order, usage.files.omitted() > 0) {
        (UsageOrder::NearestDeclaration, true) => FIND_USAGE_NEAREST.render(shown),
        (UsageOrder::NearestDeclaration, false) => FIND_USAGE_NEAREST_ALL.to_owned(),
        (UsageOrder::MostLines, true) => FIND_USAGE_LINES.render(shown),
        (UsageOrder::MostLines, false) => FIND_USAGE_LINES_ALL.to_owned(),
    };
    format!("{head} {order}")
}

fn usage_row(file: &UsageFile) -> String {
    let (line, lines) = (file.line as usize, file.lines as usize);
    FIND_USAGE_ROW.render((&one_line(&file.path), line, lines))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    data_kind_excluded: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outside_outline: Option<OutsideJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<UsageJson<'a>>,
}

// The code files that spell the name and do not declare it: the owner only for a qualified query.
#[derive(Clone, Serialize)]
struct UsageJson<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'a str>,
    total: usize,
    limit: usize,
    order: &'static str,
    files: Vec<UsageFileJson<'a>>,
}

#[derive(Clone, Serialize)]
struct UsageFileJson<'a> {
    path: &'a str,
    line: u32,
    lines: u32,
}

impl<'a> UsageJson<'a> {
    fn of(usage: &'a Usage) -> Self {
        let files = usage.files.rows().iter().map(|file| UsageFileJson {
            path: &file.path,
            line: file.line,
            lines: file.lines,
        });
        UsageJson {
            name: &usage.name,
            owner: usage.owner.as_deref(),
            total: usage.files.total(),
            limit: usage.files.limit(),
            order: match usage.order {
                UsageOrder::NearestDeclaration => "nearest_declaration",
                UsageOrder::MostLines => "most_lines",
            },
            files: files.collect(),
        }
    }
}

#[derive(Clone, Serialize)]
struct PlaceJson<'a> {
    path: &'a str,
    line: u32,
}

// The data files that spell the query outside their outline, and the files the budget left.
#[derive(Clone, Serialize)]
struct OutsideJson<'a> {
    places: Vec<PlaceJson<'a>>,
    unread: usize,
    budget: usize,
}

impl<'a> OutsideJson<'a> {
    fn of(outside: &'a OutsideOutline) -> Option<Self> {
        if outside.places.is_empty() && outside.unread == 0 {
            return None;
        }
        let places = outside.places.iter();
        Some(OutsideJson {
            places: places
                .map(|(path, line)| PlaceJson { path, line: *line })
                .collect(),
            unread: outside.unread,
            budget: outside.budget,
        })
    }
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
                DataEntry::Folded(file) | DataEntry::FoldedRest(file) => {
                    folded.push(FoldedJson::of(file));
                }
            }
        }
        folded.extend(result.data.unlisted.iter().map(FoldedJson::of));
        let total = cards.total() + result.data.total();
        let excluded = result.data_kind_excluded;
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
            data_kind_excluded: (excluded > 0).then_some(excluded),
            outside_outline: OutsideJson::of(&result.outside),
            usage: result.usage.as_ref().map(UsageJson::of),
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
        let counts = (result.unresolved_omitted, result.unresolved_total());
        rows.push(EXPAND_UNRESOLVED_UNLISTED.render(counts));
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
