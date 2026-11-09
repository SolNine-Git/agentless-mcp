//! The lines of one file that a search shows: each match line and the context lines around it,
//! merged in line order, each with its number. Only a minified file's lines are cut.

use regex::Regex;

use super::Line;
use super::files::is_source_path;
use super::noise::{NoiseClass, class_of, cut, is_minified};

// One file with a match: its class, the lines the answer may show, and its match count.
#[derive(Debug)]
pub(crate) struct FileHits {
    pub(super) path: String,
    pub(super) source: bool,
    pub(super) class: Option<NoiseClass>,
    pub(super) lines: Vec<Line>,
    pub(super) matches: usize,
}

// The hits of `text`, the file at `path`, or `None` when no line matches.
pub(super) fn hits_in(path: &str, text: &str, regex: &Regex, context: usize) -> Option<FileHits> {
    let all = text_lines(text);
    let matched: Vec<usize> = (all.iter().enumerate())
        .filter(|(_, line)| regex.is_match(line))
        .map(|(at, _)| at)
        .collect();
    if matched.is_empty() {
        return None;
    }
    let minified = is_minified(path, &all);
    let windows = windows(&matched, context, all.len());
    let lines = (windows.into_iter())
        .flat_map(|(first, last)| first..=last)
        .map(|at| line(at, all[at], matched.binary_search(&at).is_ok(), minified))
        .collect();
    Some(FileHits {
        path: path.to_owned(),
        source: is_source_path(path),
        class: class_of(path, minified),
        lines,
        matches: matched.len(),
    })
}

// The parts that "\n" or "\r\n" ends: a final line break opens no line, as a reader counts lines.
fn text_lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

// Each match widened by `context` on both sides, with touching or overlapping windows merged.
fn windows(matched: &[usize], context: usize, count: usize) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for &at in matched {
        let first = at.saturating_sub(context);
        let last = at.saturating_add(context).min(count - 1);
        match merged.last_mut() {
            Some((_, end)) if first <= end.saturating_add(1) => *end = last,
            _ => merged.push((first, last)),
        }
    }
    merged
}

fn line(at: usize, text: &str, matched: bool, minified: bool) -> Line {
    let number = u32::try_from(at + 1).unwrap_or(u32::MAX);
    let (text, cut) = if minified {
        cut(text)
    } else {
        (text.to_owned(), None)
    };
    Line {
        number,
        text,
        matched,
        cut,
    }
}
