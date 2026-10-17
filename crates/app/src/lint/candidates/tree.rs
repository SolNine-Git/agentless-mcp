//! The pair rows of a candidate: each edited file's working-tree text before the edits and after.

use std::collections::BTreeMap;
use std::path::Path;

use checks::{TextPair, pair_language};
use patch::{Edit, EditOutcome, apply_edits};
use prompts::messages::{PAIR_EDITS_UNAPPLIED, PAIR_OLD_UNREAD};
use syntax::Language;

use crate::pairs::{IndexOf, Judged, Reported, run_pairs};
use crate::proposed::working_read;
use crate::rows::{Gap, coverage, side_text};
use crate::{Error, PairSelection};

// One working-tree read: `None` for a file that the change creates, which is not a file that
// could not be read.
pub(super) type WorkingRead = Option<Result<Vec<u8>, String>>;

// The shared clone index of the run, and the functions near_duplicates reported.
pub(super) struct Run<'a, 'i> {
    pub(super) index: &'a mut IndexOf<'i>,
    pub(super) reported: &'a Reported,
}

// The edited files the scan would read, so orientation without a scan judges the same files.
pub(super) fn scanned_paths(edits: &[Edit]) -> Vec<&str> {
    let scanned = |path: &&str| *path != io::CONFIG_FILENAME && Language::from_path(path).is_some();
    edited_paths(edits).into_iter().filter(scanned).collect()
}

// The edited files a pair check reads.
pub(super) fn pair_paths(edits: &[Edit]) -> Vec<&str> {
    let paired = |path: &&str| pair_language(path).is_some();
    edited_paths(edits).into_iter().filter(paired).collect()
}

pub(super) fn working_reads(root: &Path, paths: &[&str]) -> BTreeMap<String, WorkingRead> {
    paths
        .iter()
        .map(|path| ((*path).to_owned(), working_read(&root.join(path))))
        .collect()
}

// The texts of the reads that read, decoded with U+FFFD.
pub(super) fn lenient_texts(reads: &BTreeMap<String, WorkingRead>) -> BTreeMap<String, String> {
    reads
        .iter()
        .filter_map(|(path, read)| {
            let bytes = read.as_ref()?.as_ref().ok()?;
            Some((path.clone(), String::from_utf8_lossy(bytes).into_owned()))
        })
        .collect()
}

// Each edited file in a pair language is one pair, or the row that says why it is none. `reads`
// holds every such file, and each read moves into its pair.
pub(super) fn pair_rows(
    edits: &[Edit],
    reads: &mut BTreeMap<String, WorkingRead>,
    selection: PairSelection,
    config: &io::ProjectConfig,
    Run { index, reported }: Run<'_, '_>,
) -> Result<Judged, Error> {
    let mut rows = Vec::new();
    let mut pairs = Vec::new();
    for (path, path_edits) in by_path(edits) {
        if pair_language(path).is_none() {
            continue;
        }
        let read = reads.remove(path).expect("every pair path was read");
        match candidate_pair(path, read, &path_edits) {
            Ok(pair) => pairs.push(pair),
            Err(row) => rows.push(*row),
        }
    }
    let judged = run_pairs(pairs, selection, config, index, reported)?;
    rows.extend(judged.rows);
    Ok(Judged {
        rows,
        observed: judged.observed,
    })
}

// The paths in the order of their first edit, each once.
fn edited_paths(edits: &[Edit]) -> Vec<&str> {
    by_path(edits).into_iter().map(|(path, _)| path).collect()
}

// The edits of each path, the paths in the order of their first edit.
fn by_path(edits: &[Edit]) -> Vec<(&str, Vec<Edit>)> {
    let mut grouped: Vec<(&str, Vec<Edit>)> = Vec::new();
    for edit in edits {
        match grouped.iter_mut().find(|(path, _)| *path == edit.path) {
            Some((_, group)) => group.push(edit.clone()),
            None => grouped.push((&edit.path, vec![edit.clone()])),
        }
    }
    grouped
}

fn candidate_pair(path: &str, read: WorkingRead, edits: &[Edit]) -> Result<TextPair, Gap> {
    let old = match read {
        Some(read) => Some(side_text(&PAIR_OLD_UNREAD, path, read)?),
        None => None,
    };
    let (mut contents, pending) = initial(path, old.as_deref(), edits);
    let applied = apply_edits(pending, &contents);
    let failures: Vec<&EditOutcome> = applied.failures().collect();
    if let Some(first) = failures.first() {
        let message = PAIR_EDITS_UNAPPLIED.render((path, failures.len()));
        return Err(Box::new(coverage(message, path, first.reason.clone())));
    }
    let changed = applied
        .new_contents
        .into_iter()
        .find(|(changed, _)| changed == path);
    let new = changed
        .map(|(_, text)| text)
        .or_else(|| contents.remove(path));
    Ok(TextPair {
        path: path.to_owned(),
        old,
        new,
    })
}

// A file the change creates starts as the replace side of its first edit when that edit
// searches for nothing.
fn initial<'e>(
    path: &str,
    old: Option<&str>,
    edits: &'e [Edit],
) -> (BTreeMap<String, String>, &'e [Edit]) {
    let file = |text: &str| BTreeMap::from([(path.to_owned(), text.to_owned())]);
    match (old, edits.split_first()) {
        (Some(text), _) => (file(text), edits),
        (None, Some((first, rest))) if first.search.is_empty() => (file(&first.replace), rest),
        (None, _) => (BTreeMap::new(), edits),
    }
}
