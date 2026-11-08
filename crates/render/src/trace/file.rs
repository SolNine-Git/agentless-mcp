//! The file card's text, with its declarations, imports, importers, tests and import cycles, and
//! the folder card's text, with one row per entry.

use app::search::{FileRow, Measure};
use app::trace::{
    Cycles, Declares, FileLinks, FileTrace, FolderEntry, FolderTrace, Linked, Section, Tally,
};
use app::{CycleRow, ROW_INDENT, SymbolCard};
use facts::{StableId, split_ordinal};
use prompts::messages::{
    LISTING_ROW, LISTING_ROW_SYMBOLS, LISTING_ROW_UNREAD, TARGET_NO_LINE, TRACE_CYCLE_MEMBERS,
    TRACE_CYCLES, TRACE_DECLARES, TRACE_FILE_HEADER, TRACE_FOLDER_HEADER, TRACE_FOLDER_ROW,
    TRACE_MORE_HINT, TRACE_NO_ANCHOR, TRACE_NO_SYMBOL_AT, TRACE_PART_HIDDEN, TRACE_ROWS_UNLISTED,
    TRACE_SECTION_UNREAD, TRACE_TREE_UNLISTED,
};
use text::{is_word, one_line};

use super::symbol::change_line;
use super::{head, is_cut, item_lines, none, tally};
use crate::cycles::chain;
use crate::views::QUALIFIED_NAME;

// The folder that a folder card of the root names.
const ROOT: &str = ".";

pub(super) fn file_lines(file: &FileTrace, more: Option<Section>) -> Vec<String> {
    let path = one_line(&file.path);
    let mut lines = Vec::new();
    if let Some(line) = file.no_symbol_at {
        let note = TARGET_NO_LINE.render((&path, line as usize));
        lines.push(format!("{note}{TRACE_NO_SYMBOL_AT}"));
    }
    let pattern = StableId::new(file.id_prefix, &file.path, QUALIFIED_NAME).to_string();
    let header = (
        &*path,
        file.language,
        &*one_line(&pattern),
        file.lines as usize,
    );
    lines.push(TRACE_FILE_HEADER.render(header));
    lines.extend((file.declares.as_ref()).map_or_else(Vec::new, |found| declares(found, more)));
    let sides = [
        (Section::Imports, &file.imports, false),
        (Section::Importers, &file.importers, true),
        (Section::Tests, &file.tests, true),
    ];
    for (section, linked, at_line) in sides {
        if let Some(linked) = linked {
            lines.extend(linked_lines(section, linked, at_line, more));
        }
    }
    lines.extend((file.cycles.as_ref()).map_or_else(Vec::new, |found| cycles(found, more)));
    // A file card has no history section for a hint to name.
    lines.extend((file.change.as_ref()).map(|change| change_line(change, &file.path, None)));
    lines
}

// The count of each kind, the kinds that the cap hid, and the kept declarations in line order.
fn declares(declares: &Declares, more: Option<Section>) -> Vec<String> {
    if declares.total == 0 {
        return vec![none(Section::Declares)];
    }
    let kinds: Vec<String> = (declares.kinds.iter())
        .map(|(kind, count)| counted(kind, *count))
        .collect();
    let mut parts = vec![TRACE_DECLARES.render((&kinds.join(", "), declares.total))];
    let hidden: Vec<String> = (declares.kinds.iter())
        .filter_map(|(kind, count)| {
            let kept = (declares.rows.iter())
                .filter(|row| row.kind == *kind)
                .count();
            (kept < *count).then(|| counted(kind, count - kept))
        })
        .collect();
    if !hidden.is_empty() {
        parts.push(TRACE_PART_HIDDEN.render(&hidden.join(", ")));
    }
    let mut lines = vec![head(Section::Declares, &parts, more, !hidden.is_empty())];
    let rows: Vec<String> = declares.rows.iter().map(declared).collect();
    lines.extend(item_lines(&rows));
    lines
}

// A count and its kind, such as `2 type aliases`.
fn counted(kind: &str, count: usize) -> String {
    let word = one_line(kind).replace('_', " ");
    if count == 1 {
        format!("{count} {word}")
    } else if word.ends_with('s') {
        format!("{count} {word}es")
    } else {
        format!("{count} {word}s")
    }
}

// The signature up to the declared name, with the qualified name in its place so that the row
// joins the id pattern of the header.
fn declared(card: &SymbolCard) -> String {
    let parsed = card.stable_id.parse::<StableId>();
    let qualname = parsed
        .as_ref()
        .map_or(card.stable_id.as_str(), StableId::qualname);
    let (named, _) = split_ordinal(qualname);
    let name = named.rsplit('.').next().unwrap_or(named);
    let signature = card.signature.as_str();
    let declared = match word_at(signature, name) {
        Some(at) => format!("{}{qualname}", &signature[..at]),
        None => format!("{} {qualname}", card.kind),
    };
    format!("{} @{}", one_line(&declared), card.start_line)
}

// The first place where `word` stands in `text` with no word character on either side.
fn word_at(text: &str, word: &str) -> Option<usize> {
    let alone = |at: usize| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    };
    (!word.is_empty())
        .then(|| {
            text.match_indices(word)
                .map(|(at, _)| at)
                .find(|at| alone(*at))
        })
        .flatten()
}

// An import's line is in the importing file, so only the importers and the tests print it.
fn linked_lines(
    section: Section,
    linked: &Linked<FileLinks>,
    at_line: bool,
    more: Option<Section>,
) -> Vec<String> {
    let found = match linked {
        Linked::NoAnchor => {
            return vec![TRACE_SECTION_UNREAD.render((section.as_str(), TRACE_NO_ANCHOR))];
        }
        Linked::Found(found) if found.total == Tally::Exact(0) => return vec![none(section)],
        Linked::Found(found) => found,
    };
    let hidden = is_cut(found.rows.len(), found.total);
    let mut lines = vec![head(section, &[tally(found.total)], more, hidden)];
    let items: Vec<String> = (found.rows.iter())
        .map(|place| {
            let path = one_line(&place.path);
            if at_line {
                format!("{path} @{}", place.line)
            } else {
                path.into_owned()
            }
        })
        .collect();
    lines.extend(item_lines(&items));
    lines
}

// A file is in one strongly connected component at most, whose chain the card prints; `more`
// adds the members off the chain.
fn cycles(cycles: &Cycles, more: Option<Section>) -> Vec<String> {
    let Some(first) = cycles.rows.first() else {
        return vec![none(Section::Cycles)];
    };
    let count = match cycles.total {
        Tally::Exact(count) | Tally::Floor(count) => count,
    };
    let part = TRACE_CYCLES.render((count, first.members.len()));
    let hidden = first.members.len() > first.files.len() || is_cut(cycles.rows.len(), cycles.total);
    let mut lines = vec![head(Section::Cycles, &[part], more, hidden)];
    for cycle in &cycles.rows {
        lines.push(format!("{ROW_INDENT}{}", one_line(&chain(cycle))));
        if more.is_some() {
            lines.extend(off_chain(cycle));
        }
    }
    lines
}

fn off_chain(cycle: &CycleRow) -> Option<String> {
    let off: Vec<String> = (cycle.members.iter())
        .filter(|member| !cycle.files.contains(member))
        .map(|member| one_line(member).into_owned())
        .collect();
    let listed = TRACE_CYCLE_MEMBERS.render(&off.join(", "));
    (!off.is_empty()).then(|| format!("{ROW_INDENT}{listed}"))
}

// The entries in tree order, each indented by its level and named by its last part.
pub(super) fn folder_lines(folder: &FolderTrace, more: Option<Section>) -> Vec<String> {
    let path = if folder.path.is_empty() {
        ROOT
    } else {
        &folder.path
    };
    let lines_read = usize::try_from(folder.lines).unwrap_or(usize::MAX);
    let header = (&*one_line(path), folder.files, lines_read);
    let mut lines = vec![TRACE_FOLDER_HEADER.render(header)];
    lines.extend(folder.entries.iter().map(entry_line));
    let rest = folder.total.saturating_sub(folder.entries.len());
    if rest > 0 {
        let line = match more {
            None => {
                let hint = TRACE_MORE_HINT.render(Section::Tree.as_str());
                format!("{}  {hint}", TRACE_ROWS_UNLISTED.render(rest))
            }
            Some(_) => TRACE_TREE_UNLISTED.render(rest),
        };
        lines.push(format!("{ROW_INDENT}{line}"));
    }
    lines
}

fn entry_line(entry: &FolderEntry) -> String {
    match entry {
        FolderEntry::File { depth, row } => {
            let named = FileRow {
                path: last_part(&row.path).to_owned(),
                measure: row.measure.clone(),
            };
            format!("{}{}", ROW_INDENT.repeat(*depth), listing_row(&named))
        }
        FolderEntry::Folder { depth, path, files } => {
            let row = TRACE_FOLDER_ROW.render((&one_line(last_part(path)), *files));
            format!("{}{row}", ROW_INDENT.repeat(*depth))
        }
    }
}

fn last_part(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// A file's row in a folder card or a listing: its path, its lines, and its symbols when a grammar
// read it, or why it was not read.
pub(crate) fn listing_row(row: &FileRow) -> String {
    let path = one_line(&row.path);
    match &row.measure {
        Measure::Counted {
            lines,
            symbols: Some(symbols),
        } => LISTING_ROW_SYMBOLS.render((&path, *lines as usize, *symbols)),
        Measure::Counted {
            lines,
            symbols: None,
        } => LISTING_ROW.render((&path, *lines as usize)),
        Measure::Unread(reason) => LISTING_ROW_UNREAD.render((&path, &one_line(reason))),
    }
}
