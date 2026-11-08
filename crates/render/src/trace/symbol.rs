//! The symbol card's text: the definition, then callers, callees, tests, similar functions and the
//! last change, or one section in full.

use app::trace::{Callees, Callers, Change, Section, Similar, SymbolTrace, Tally, Tests};
use app::{CoverageStatus, HistoryResult, ROW_INDENT, RefGroup, SymbolCard, locator};
use prompts::messages::{
    TRACE_BOUND, TRACE_HIDDEN, TRACE_LAST_CHANGE, TRACE_LAST_CHANGE_NOT_READ,
    TRACE_LAST_CHANGE_UNAVAILABLE, TRACE_MORE_HINT, TRACE_NAME_ONLY_FILES,
    TRACE_NAME_ONLY_FILES_FLOOR, TRACE_NAME_ONLY_HIDDEN, TRACE_NOT_RESOLVED, TRACE_SECTION_UNREAD,
    TRACE_SIMILAR, TRACE_SIMILAR_NAME_ONLY, TRACE_SIMILAR_ROW, TRACE_TIER_UNCHECKED,
};
use text::one_line;

use super::{head, is_cut, none, span, tally, tier_word, unlisted};
use crate::history::history_text;

// The characters of a strict ISO 8601 author date that name the day.
const DAY: usize = 10;

pub(super) fn symbol_lines(traced: &SymbolTrace, more: Option<Section>) -> Vec<String> {
    let card = &traced.card;
    let mut lines = vec![header(card)];
    match &traced.body {
        Some(Ok(body)) => {
            let numbered = body.body.split('\n');
            lines.extend(numbered.map(|line| format!("{ROW_INDENT}{}", one_line(line))));
            return lines;
        }
        Some(Err(reason)) => {
            lines.push(signature(card));
            lines.push(unread(Section::Body, reason));
            return lines;
        }
        None => lines.push(signature(card)),
    }
    if let Some(callers) = &traced.callers {
        lines.extend(callers_lines(callers, more));
    }
    if let Some(callees) = &traced.callees {
        lines.extend(callees_lines(callees, more));
    }
    if let Some(tests) = &traced.tests {
        lines.extend(tests_lines(tests, more));
    }
    if let Some(similar) = &traced.similar {
        lines.extend(similar_lines(similar, more));
    }
    lines.extend(traced.callers.as_ref().and_then(not_resolved));
    lines.extend((traced.change.as_ref()).map(|change| change_line(change, more)));
    lines.extend(
        traced
            .history
            .as_ref()
            .map(history_lines)
            .unwrap_or_default(),
    );
    lines
}

fn header(card: &SymbolCard) -> String {
    let at = span(card.start_line, card.end_line);
    let id = locator(&card.stable_id, "", Some(&at));
    format!("{id}  {}", one_line(card.kind))
}

fn signature(card: &SymbolCard) -> String {
    format!("{ROW_INDENT}{}", one_line(&card.signature))
}

fn unread(section: Section, reason: &str) -> String {
    TRACE_SECTION_UNREAD.render((section.as_str(), &one_line(reason)))
}

// The binding sites print as rows; the name-only sites print as a count.
fn callers_lines(callers: &Callers, more: Option<Section>) -> Vec<String> {
    if callers.bound + callers.name_only == 0 {
        return vec![none(Section::Callers)];
    }
    let mut parts = vec![TRACE_BOUND.render(callers.bound)];
    if callers.name_only > 0 {
        parts.push(match callers.name_only_files {
            Tally::Exact(files) => TRACE_NAME_ONLY_FILES.render((callers.name_only, files)),
            Tally::Floor(files) => TRACE_NAME_ONLY_FILES_FLOOR.render((callers.name_only, files)),
        });
    }
    let hidden = callers.name_only > 0 || is_cut(callers.rows.len(), callers.rows_total);
    let mut lines = vec![head(Section::Callers, &parts, more, hidden)];
    lines.extend(callers.rows.iter().map(group_row));
    lines.extend(unlisted(callers.rows.len(), callers.rows_total));
    lines
}

// One file at one tier, with each site at the symbol that holds it.
fn group_row(group: &RefGroup) -> String {
    let tier = tier_word(group.tier);
    let tier = if group.scope_checked {
        tier
    } else {
        TRACE_TIER_UNCHECKED.render(&tier)
    };
    let sites: Vec<String> = (group.sites.iter())
        .map(|site| format!("{} @{}", one_line(&site.enclosing), site.line))
        .collect();
    let path = one_line(&group.path);
    format!("{ROW_INDENT}{path}  {tier}  {}", sites.join(", "))
}

fn callees_lines(callees: &Callees, more: Option<Section>) -> Vec<String> {
    if callees.bound + callees.name_only == 0 {
        return vec![none(Section::Callees)];
    }
    let mut parts = vec![TRACE_BOUND.render(callees.bound)];
    if callees.name_only > 0 {
        parts.push(TRACE_NAME_ONLY_HIDDEN.render(callees.name_only));
    }
    let hidden = callees.name_only > 0 || is_cut(callees.rows.len(), callees.rows_total);
    let mut lines = vec![head(Section::Callees, &parts, more, hidden)];
    lines.extend(callees.rows.iter().map(|group| {
        let edges: Vec<String> = (group.edges.iter())
            .map(|edge| format!("{} @{}", one_line(&edge.label), edge.line))
            .collect();
        let (path, tier) = (one_line(&group.path), tier_word(group.tier));
        format!("{ROW_INDENT}{path}  {tier}  {}", edges.join(", "))
    }));
    lines.extend(unlisted(callees.rows.len(), callees.rows_total));
    lines
}

fn tests_lines(tests: &Tests, more: Option<Section>) -> Vec<String> {
    if tests.total == Tally::Exact(0) {
        return vec![none(Section::Tests)];
    }
    let hidden = is_cut(tests.rows.len(), tests.total);
    let mut lines = vec![head(Section::Tests, &[tally(tests.total)], more, hidden)];
    lines.extend(tests.rows.iter().map(group_row));
    lines
}

// A candidate that shares no bound caller is not ranked, so with no ranked candidate the section
// is its count.
fn similar_lines(similar: &Similar, more: Option<Section>) -> Vec<String> {
    if similar.total == 0 {
        return vec![none(Section::Similar)];
    }
    if similar.rows.is_empty() {
        let part = TRACE_SIMILAR_NAME_ONLY.render(similar.total);
        return vec![head(Section::Similar, &[part], more, true)];
    }
    let hidden = similar.total - similar.rows.len();
    let mut parts = vec![TRACE_SIMILAR.render(similar.total)];
    if hidden > 0 {
        parts.push(TRACE_HIDDEN.render(hidden));
    }
    let mut lines = vec![head(Section::Similar, &parts, more, hidden > 0)];
    lines.extend(similar.rows.iter().map(|row| {
        let line = row.line.to_string();
        let at = locator(&row.stable_id, "", Some(&line));
        format!(
            "{ROW_INDENT}{}",
            TRACE_SIMILAR_ROW.render((&at, row.overlap))
        )
    }));
    lines
}

// Only a fan-in that read every code file vouches that no caller is missing.
fn not_resolved(callers: &Callers) -> Option<String> {
    (callers.coverage.status == CoverageStatus::Partial)
        .then(|| TRACE_NOT_RESOLVED.render(callers.coverage.skipped_code_files))
}

// The commit, or why git gave none. A default card points at the history that `more` reads.
pub(super) fn change_line(change: &Change, more: Option<Section>) -> String {
    let hinted = |line: String| match more {
        None => format!(
            "{line}  {}",
            TRACE_MORE_HINT.render(Section::History.as_str())
        ),
        Some(_) => line,
    };
    match change {
        Change::Commit(entry) => {
            let day = entry.authored.get(..DAY).unwrap_or(&entry.authored);
            let (sha, day, subject) = (
                one_line(entry.short_sha()),
                one_line(day),
                one_line(&entry.subject),
            );
            hinted(TRACE_LAST_CHANGE.render((&sha, &day, &subject)))
        }
        Change::NotRead { seconds } => {
            let seconds = usize::try_from(*seconds).unwrap_or(usize::MAX);
            hinted(TRACE_LAST_CHANGE_NOT_READ.render(seconds))
        }
        Change::Unavailable(reason) => TRACE_LAST_CHANGE_UNAVAILABLE.render(&one_line(reason)),
    }
}

fn history_lines(history: &Result<HistoryResult, String>) -> Vec<String> {
    match history {
        Ok(result) => (history_text(result).lines()).map(str::to_owned).collect(),
        Err(reason) => vec![unread(Section::History, reason)],
    }
}
