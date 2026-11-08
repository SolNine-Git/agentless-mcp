//! The answers with no symbol, file or folder card: the candidates of a name, a miss with the next
//! call to make, and a `more` section that the card does not hold.

use app::search::Place;
use app::trace::{CandidatesTrace, CardKind, Miss, Section};
use app::{ROW_INDENT, Usage, locator};
use prompts::messages::{
    TARGET_NO_LINE, TRACE_CANDIDATES, TRACE_LINE_PAST_END, TRACE_LITERAL, TRACE_LITERAL_NONE,
    TRACE_MORE_HINT, TRACE_NO_GRAMMAR, TRACE_NO_GRAMMAR_NAME, TRACE_NO_MATCH,
    TRACE_NO_SECTION_FILE, TRACE_NO_SECTION_FOLDER, TRACE_NO_SECTION_SYMBOL, TRACE_NOT_PARSED,
    TRACE_NOT_SCANNED, TRACE_ROWS_UNLISTED, TRACE_UNKNOWN_PATH, TRACE_UNKNOWN_PATH_NONE,
};
use text::{one_line, posix_name, split_suffix};

use super::span;
use crate::symbols::usage_text;

pub(super) fn candidates_lines(
    candidates: &CandidatesTrace,
    target: &str,
    more: Option<Section>,
) -> Vec<String> {
    let mut lines = vec![TRACE_CANDIDATES.render((&one_line(target), candidates.total))];
    lines.extend(candidates.rows.iter().map(|card| {
        let at = span(card.start_line, card.end_line);
        let id = locator(&card.stable_id, "", Some(&at));
        format!("{ROW_INDENT}{id}  {}", one_line(card.kind))
    }));
    let rest = candidates.total.saturating_sub(candidates.rows.len());
    if rest > 0 {
        let mut line = format!("{ROW_INDENT}{}", TRACE_ROWS_UNLISTED.render(rest));
        if more.is_none() {
            line.push_str("  ");
            line.push_str(&TRACE_MORE_HINT.render(Section::Candidates.as_str()));
        }
        lines.push(line);
    }
    lines
}

// Each miss names the next useful call: the files that use the name, where its text occurs, or
// what to read in place of a card.
pub(super) fn miss_lines(miss: &Miss, target: &str) -> Vec<String> {
    match miss {
        Miss::Nothing { usage, literal, .. } => {
            nothing_lines(usage.as_ref(), literal.as_ref(), target)
        }
        Miss::UnknownPath { path, endings } => vec![unknown_path(path, endings)],
        Miss::NotParsed { path, reason } => {
            vec![TRACE_NOT_PARSED.render((&one_line(path), &one_line(reason)))]
        }
        Miss::NoGrammar { path } => vec![no_grammar(path)],
        Miss::NotScanned { path } => vec![TRACE_NOT_SCANNED.render(&one_line(path))],
        Miss::LinePastEnd { path, line, lines } => {
            let note = TARGET_NO_LINE.render((&one_line(path), *line as usize));
            vec![note + &TRACE_LINE_PAST_END.render(*lines as usize)]
        }
    }
}

fn nothing_lines(
    usage: Option<&Usage>,
    literal: Option<&(Vec<Place>, usize)>,
    target: &str,
) -> Vec<String> {
    let head = TRACE_NO_MATCH.render(&one_line(target));
    match (usage, literal) {
        (Some(usage), _) => {
            let mut lines = vec![head];
            lines.extend(usage_text(usage).lines().map(str::to_owned));
            lines
        }
        (None, Some((places, total))) => {
            vec![format!("{head} {}", literal_text(places, *total, target))]
        }
        (None, None) => vec![head],
    }
}

// The target's text, which the literal scan read, is the pattern that lists every match.
fn literal_text(places: &[Place], total: usize, target: &str) -> String {
    if total == 0 {
        return TRACE_LITERAL_NONE.to_owned();
    }
    let listed: Vec<String> = (places.iter())
        .map(|place| format!("{} @{}", one_line(&place.path), place.line))
        .collect();
    TRACE_LITERAL.render((&one_line(target), total, &listed.join(", ")))
}

fn unknown_path(path: &str, endings: &[String]) -> String {
    let path = one_line(path);
    if endings.is_empty() {
        return TRACE_UNKNOWN_PATH_NONE.render(&path);
    }
    let endings: Vec<String> = (endings.iter())
        .map(|ending| one_line(ending).into_owned())
        .collect();
    TRACE_UNKNOWN_PATH.render((&path, &endings.join(", ")))
}

fn no_grammar(path: &str) -> String {
    let (_, suffix) = split_suffix(posix_name(path));
    if suffix.is_empty() {
        TRACE_NO_GRAMMAR_NAME.render(&one_line(path))
    } else {
        TRACE_NO_GRAMMAR.render((&one_line(path), &one_line(suffix)))
    }
}

// Trace refuses an absent section only on the symbol, file and folder cards; a candidates card
// and a miss answer whatever `more` names.
pub(super) fn no_section(section: Section, kind: CardKind) -> String {
    let listed: Vec<&str> = kind.sections().iter().map(|held| held.as_str()).collect();
    let template = match kind {
        CardKind::Symbol => &TRACE_NO_SECTION_SYMBOL,
        CardKind::File => &TRACE_NO_SECTION_FILE,
        CardKind::Folder => &TRACE_NO_SECTION_FOLDER,
        CardKind::Candidates | CardKind::Miss => {
            unreachable!("trace refuses no section on a {} card", kind.as_str())
        }
    };
    template.render((section.as_str(), &listed.join(", ")))
}
