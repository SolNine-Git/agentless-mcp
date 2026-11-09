//! The answers with no symbol, file or folder card: the candidates of a name, a miss with the next
//! call to make, and a `more` section that the card does not hold.

use app::search::Place;
use app::trace::{CandidatesTrace, CardKind, Miss, Revision, Section};
use app::{ROW_INDENT, Usage};
use prompts::messages::{
    TARGET_NO_LINE, TRACE_BODY_OF_FILE, TRACE_CANDIDATES, TRACE_LINE_PAST_END, TRACE_LITERAL,
    TRACE_LITERAL_NONE, TRACE_NO_GRAMMAR, TRACE_NO_GRAMMAR_NAME, TRACE_NO_MATCH,
    TRACE_NO_SECTION_FILE, TRACE_NO_SECTION_FOLDER, TRACE_NO_SECTION_SYMBOL, TRACE_NOT_PARSED,
    TRACE_NOT_SCANNED, TRACE_RANGE_BINARY, TRACE_RANGE_PAST_END, TRACE_RANGE_REVERSED,
    TRACE_REV_NO_FILE, TRACE_REV_NO_MATCH, TRACE_ROWS_UNLISTED, TRACE_UNKNOWN_PATH,
    TRACE_UNKNOWN_PATH_NONE,
};
use text::{one_line, posix_name, split_suffix};

use super::{More, card_row};
use crate::symbols::usage_text;

// io's reason for a file over the cap starts with the word that the miss sentence already says.
const SKIPPED_WORD: &str = "skipped: ";

pub(super) fn candidates_lines(
    candidates: &CandidatesTrace,
    target: &str,
    more: More<'_>,
) -> Vec<String> {
    let mut lines = vec![TRACE_CANDIDATES.render((&one_line(target), candidates.total))];
    lines.extend(candidates.rows.iter().map(card_row));
    let rest = candidates.total.saturating_sub(candidates.rows.len());
    if rest > 0 {
        let mut line = format!("{ROW_INDENT}{}", TRACE_ROWS_UNLISTED.render(rest));
        if more.is_none() {
            line.push_str("  ");
            line.push_str(&more.hint(Section::Candidates));
        }
        lines.push(line);
    }
    lines
}

// Each miss names the next useful call: the files that use the name, where its text occurs, or
// what to read in place of a card. A miss at a revision names the commit it read.
pub(super) fn miss_lines(miss: &Miss, target: &str, revision: Option<&Revision>) -> Vec<String> {
    match miss {
        Miss::Nothing { usage, literal, .. } => {
            let head = no_match(target, revision);
            nothing_lines(head, usage.as_ref(), literal.as_ref(), target)
        }
        Miss::AbsentAtRevision { path } => {
            let revision = revision.expect("only a trace at a revision lacks a path");
            let at = (&*one_line(&revision.name), revision.short());
            vec![TRACE_REV_NO_FILE.render((&one_line(path), at.0, at.1))]
        }
        Miss::UnknownPath { path, endings } => vec![unknown_path(path, endings)],
        Miss::NotParsed { path, reason } => {
            let reason = reason.strip_prefix(SKIPPED_WORD).unwrap_or(reason);
            vec![TRACE_NOT_PARSED.render((&one_line(path), &one_line(reason)))]
        }
        Miss::NoGrammar { path } => vec![no_grammar(path)],
        Miss::NotScanned { path } => vec![TRACE_NOT_SCANNED.render(&one_line(path))],
        Miss::LinePastEnd { path, line, lines } => {
            let note = TARGET_NO_LINE.render((&one_line(path), *line as usize));
            vec![note + &TRACE_LINE_PAST_END.render(*lines as usize)]
        }
        Miss::RangePastEnd {
            path,
            start,
            end,
            lines,
        } => {
            let note =
                TRACE_RANGE_PAST_END.render((&one_line(path), *start as usize, *end as usize));
            vec![note + &TRACE_LINE_PAST_END.render(*lines as usize)]
        }
        Miss::BadRange { path, start, end } => {
            let range = (&*one_line(path), *start as usize, *end as usize);
            vec![TRACE_RANGE_REVERSED.render(range)]
        }
        Miss::Binary { path } => vec![TRACE_RANGE_BINARY.render(&one_line(path))],
    }
}

fn no_match(target: &str, revision: Option<&Revision>) -> String {
    let target = one_line(target);
    match revision {
        None => TRACE_NO_MATCH.render(&target),
        Some(revision) => {
            let name = one_line(&revision.name);
            TRACE_REV_NO_MATCH.render((&name, revision.short(), &target))
        }
    }
}

fn nothing_lines(
    head: String,
    usage: Option<&Usage>,
    literal: Option<&(Vec<Place>, usize)>,
    target: &str,
) -> Vec<String> {
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

// Trace refuses an absent section only on the symbol, file and folder cards; a candidates card,
// a range card and a miss answer whatever `more` names.
pub(super) fn no_section(section: Section, kind: CardKind) -> String {
    let listed: Vec<&str> = kind.sections().iter().map(|held| held.as_str()).collect();
    let template = match kind {
        CardKind::Symbol => &TRACE_NO_SECTION_SYMBOL,
        CardKind::File => &TRACE_NO_SECTION_FILE,
        CardKind::Folder => &TRACE_NO_SECTION_FOLDER,
        CardKind::Candidates | CardKind::Miss | CardKind::Range => {
            unreachable!("trace refuses no section on a {} card", kind.as_str())
        }
    };
    let refused = template.render((section.as_str(), &listed.join(", ")));
    if (section, kind) == (Section::Body, CardKind::File) {
        format!("{refused} {TRACE_BODY_OF_FILE}")
    } else {
        refused
    }
}
