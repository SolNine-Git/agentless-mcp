//! The map's rows as text, which the budget search measures.

use std::collections::BTreeSet;

use git::CHURN_WINDOW_DAYS;
use json::fixed;
use prompts::messages::{MAP_FILE_UNREACHED, MAP_SYMBOLS_UNLISTED};
use text::one_line;

use super::{MapEntry, MapFile};
use crate::rowtext::{ROW_INDENT, locator, stable_ids_line};

const NESTING: &str = "    ";

/// Returns the rows of a map: each ranked file's header, id pattern and symbol rows, with the
/// line for the symbols it leaves out, a blank line between files.
#[must_use]
pub fn map_rows_text(files: &[MapFile]) -> String {
    if files.is_empty() {
        return "no ranked files\n".to_owned();
    }
    // One churn pair on every file tells no file apart, so the clause drops for two or more.
    let pairs: BTreeSet<(Option<usize>, Option<i64>)> = (files.iter())
        .map(|file| (file.commits_window, file.last_commit_days))
        .collect();
    let wallpaper = files.len() > 1 && pairs.len() == 1;
    let blocks: Vec<String> = files
        .iter()
        .map(|file| file_block(file, wallpaper))
        .collect();
    blocks.join("\n\n") + "\n"
}

fn file_block(file: &MapFile, wallpaper: bool) -> String {
    let churn = if wallpaper {
        String::new()
    } else {
        churn_clause(file)
    };
    let rank = fixed(file.rank, 4);
    let mut lines = vec![format!("{}  (rank {rank}{churn})", one_line(&file.path))];
    let pattern = stable_ids_line(file.id_prefix, &file.path);
    if !pattern.is_empty() {
        lines.push(pattern);
    }
    lines.extend(file.entries.iter().flat_map(entry_rows));
    lines.extend(omission(file));
    lines.join("\n")
}

fn churn_clause(file: &MapFile) -> String {
    let Some(commits) = file.commits_window else {
        return String::new();
    };
    let last = (file.last_commit_days).map_or_else(String::new, |days| format!(", last {days}d"));
    format!(", {commits}c/{CHURN_WINDOW_DAYS}d{last}")
}

// The row names the symbol by its qualified name; the id pattern above it spells the rest.
fn entry_rows(entry: &MapEntry) -> Vec<String> {
    let name = if entry.name.is_empty() {
        &entry.stable_id
    } else {
        &entry.name
    };
    let line = entry.line.to_string();
    let indent = NESTING.repeat(entry.depth);
    let signature = one_line(&entry.signature);
    let mut rows = vec![format!(
        "{ROW_INDENT}{indent}{signature}  {}",
        locator(name, "", Some(&line))
    )];
    let nested = NESTING.repeat(entry.depth + 1);
    rows.extend(entry.rationales.iter().map(|node| {
        let kind = one_line(node.kind).to_uppercase();
        let line = node.line.to_string();
        let located = locator(&node.stable_id, &node.parent_id, Some(&line));
        format!(
            "{ROW_INDENT}{nested}# {kind}: {}  {located}",
            one_line(&node.text)
        )
    }));
    rows
}

// No budget shows a symbol in a file the focus has no path to, so that file says so instead.
fn omission(file: &MapFile) -> Option<String> {
    let omitted = file.omitted();
    if omitted == 0 {
        None
    } else if file.reached {
        Some(MAP_SYMBOLS_UNLISTED.render(omitted))
    } else {
        Some(MAP_FILE_UNREACHED.render(omitted))
    }
}

#[cfg(test)]
mod tests;
