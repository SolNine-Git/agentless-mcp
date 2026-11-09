//! The diff card's text: the header, the files rolled up by folder or each file with
//! `more=tree`, then the changed, added, removed and moved symbols.

use app::trace::{
    DiffFile, DiffFiles, DiffMoved, DiffRow, DiffSection, DiffTrace, FileStatus, Section, Tally,
};
use app::{ROW_INDENT, locator};
use prompts::messages::{
    TRACE_DIFF_ADDED, TRACE_DIFF_BASE_SPAN, TRACE_DIFF_CHANGED, TRACE_DIFF_FILE, TRACE_DIFF_FILES,
    TRACE_DIFF_FOLDER, TRACE_DIFF_HEADER, TRACE_DIFF_MOVED, TRACE_DIFF_MOVES, TRACE_DIFF_NO_FILES,
    TRACE_DIFF_REMOVED, TRACE_DIFF_RENAMED, TRACE_DIFF_SECTION, TRACE_HIDDEN, TRACE_SECTION_NONE,
};
use text::one_line;

use super::{More, span, unlisted};

pub(super) fn diff_lines(card: &DiffTrace, more: More<'_>) -> Vec<String> {
    let mut lines = vec![header(card)];
    // `more=declares` shows the symbol sections alone, as `more` shows one section of a card.
    if more.section != Some(Section::Declares) {
        lines.extend(files_lines(&card.files, more));
    }
    let Some(symbols) = card.symbols.as_ref().filter(|_| card.files.total > 0) else {
        return lines;
    };
    let sections = [
        (TRACE_DIFF_CHANGED, &symbols.changed),
        (TRACE_DIFF_ADDED, &symbols.added),
        (TRACE_DIFF_REMOVED, &symbols.removed),
    ];
    for (name, section) in sections {
        lines.extend(section_lines(name, section, card.full, more));
    }
    lines.extend(moved_lines(&symbols.moved, card.full, more));
    lines
}

// The target's revision and path, then the merge base by its short id.
fn header(card: &DiffTrace) -> String {
    let revision = match &card.scope {
        Some(scope) => format!("{}:{scope}", card.rev),
        None => card.rev.clone(),
    };
    let sha = card.base.get(..git::SHORT_SHA_LENGTH).unwrap_or(&card.base);
    TRACE_DIFF_HEADER.render((&one_line(&revision), sha, card.commits))
}

fn files_lines(files: &DiffFiles, more: More<'_>) -> Vec<String> {
    if files.total == 0 {
        return vec![TRACE_DIFF_NO_FILES.to_owned()];
    }
    let counts = (files.added, files.removed, files.changed, files.renamed);
    let (added, removed, changed, renamed) = counts;
    let head = TRACE_DIFF_FILES.render((
        files.total,
        files.folders_total,
        added,
        removed,
        changed,
        renamed,
    ));
    if let Some(rows) = &files.rows {
        let mut lines = vec![head];
        lines.extend(rows.iter().map(file_row));
        lines.extend(unlisted(rows.len(), Tally::Exact(files.total)));
        return lines;
    }
    let hidden = files.folders_total - files.folders.len();
    let mut lines = vec![hinted(head, hidden, Section::Tree, more)];
    lines.extend(files.folders.iter().map(|folder| {
        let path = if folder.path.is_empty() {
            ".".to_owned()
        } else {
            one_line(&folder.path).into_owned()
        };
        let path = path.as_str();
        let counts = &folder.symbols;
        let row = (
            path,
            folder.files,
            counts.added,
            counts.removed,
            counts.changed,
        );
        format!("{ROW_INDENT}{}", TRACE_DIFF_FOLDER.render(row))
    }));
    lines
}

// The head of a section that a default card cut, with the count it hides and the call that
// shows the rest.
fn hinted(head: String, hidden: usize, section: Section, more: More<'_>) -> String {
    if hidden == 0 || more.is_some() {
        return head;
    }
    format!(
        "{head}; {}  {}",
        TRACE_HIDDEN.render(hidden),
        more.hint(section)
    )
}

fn file_row(row: &DiffFile) -> String {
    let status = match &row.status {
        FileStatus::Added => TRACE_DIFF_ADDED.to_owned(),
        FileStatus::Removed => TRACE_DIFF_REMOVED.to_owned(),
        FileStatus::Changed => TRACE_DIFF_CHANGED.to_owned(),
        FileStatus::Renamed(from) => TRACE_DIFF_RENAMED.render(&one_line(from)),
    };
    let counts = &row.symbols;
    let path = one_line(&row.path);
    let values = (
        &*path,
        status.as_str(),
        counts.added,
        counts.removed,
        counts.changed,
    );
    format!("{ROW_INDENT}{}", TRACE_DIFF_FILE.render(values))
}

fn section_lines(name: &str, section: &DiffSection, full: bool, more: More<'_>) -> Vec<String> {
    if section.total == 0 {
        return vec![TRACE_SECTION_NONE.render(name)];
    }
    let counts = (name, section.total, section.files, section.tests);
    let head = TRACE_DIFF_SECTION.render(counts);
    let hidden = if full {
        0
    } else {
        section.total - section.rows.len()
    };
    let mut lines = vec![hinted(head, hidden, Section::Declares, more)];
    lines.extend(section.rows.iter().map(diff_row));
    if full {
        lines.extend(unlisted(section.rows.len(), Tally::Exact(section.total)));
    }
    lines
}

// A changed row carries its span at the base, which `rev` reads back.
fn diff_row(row: &DiffRow) -> String {
    let card = &row.card;
    let mut at = locator(
        &card.stable_id,
        "",
        Some(&span(card.start_line, card.end_line)),
    );
    if let Some((start, end)) = row.base {
        at = format!("{at} {}", TRACE_DIFF_BASE_SPAN.render(&span(start, end)));
    }
    format!("{ROW_INDENT}{at}  {}", one_line(card.kind))
}

// A move names the base's id bare, since only the base holds it, and the new id as a target.
fn moved_lines(moved: &DiffMoved, full: bool, more: More<'_>) -> Vec<String> {
    if moved.total == 0 {
        return vec![TRACE_SECTION_NONE.render(TRACE_DIFF_MOVED)];
    }
    let head = TRACE_DIFF_MOVES.render((TRACE_DIFF_MOVED, moved.total));
    let hidden = if full {
        0
    } else {
        moved.total - moved.rows.len()
    };
    let mut lines = vec![hinted(head, hidden, Section::Declares, more)];
    lines.extend(moved.rows.iter().map(|row| {
        let (from, to) = (&row.from, &row.to);
        let to = locator(&to.stable_id, "", Some(&span(to.start_line, to.end_line)));
        let from_span = span(from.start_line, from.end_line);
        format!(
            "{ROW_INDENT}{} @{from_span} -> {to}",
            one_line(&from.stable_id)
        )
    }));
    if full {
        lines.extend(unlisted(moved.rows.len(), Tally::Exact(moved.total)));
    }
    lines
}
