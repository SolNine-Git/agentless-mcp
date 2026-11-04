//! The pair run: one measure per kept pair, the run of each selected check, and the clone index.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;

use checks::{IndexedFiles, Observed, Pair, PairFinding, SideError, SideText, TextPair};
use metrics::Block;
use syntax::Language;

use crate::pair_runs::{IndexOf, Reported, RunInputs};
use crate::parallel;
use crate::rows::{bounded_read, decoded};
use crate::scan;
use crate::{Error, Noted, PairSelection, Services, in_metric_scope};

// The rows of one run in check order, and what the checks measured.
pub(crate) struct Judged {
    pub(crate) rows: Vec<PairFinding>,
    pub(crate) observed: Observed,
}

// What the tag cache holds for one file's text.
pub(crate) enum Stored {
    // The stored clone blocks of this exact text.
    Hit(Vec<Block>),
    // The stored reason the file has no blocks.
    Unmeasured(String),
    // No usable row: measure the text in memory.
    Miss,
    // The rows did not read: measure the text in memory, and print this warning.
    Unreadable(String),
}

// The lookup of one file's stored blocks by path, text and language.
pub(crate) type Lookup<'a> = dyn FnMut(&str, &str, Language) -> Stored + 'a;

// Each selected check runs on the pairs its path rule keeps, in registry order. The index is
// built on the first clone_delta call that keeps a pair.
pub(crate) fn run_pairs(
    pairs: Vec<TextPair>,
    selection: PairSelection,
    config: &io::ProjectConfig,
    index: &mut IndexOf<'_>,
    reported: &Reported,
) -> Result<Judged, Error> {
    let kept: Vec<TextPair> = pairs
        .into_iter()
        .filter(|pair| selection.checks().any(|check| check.keeps(&pair.path)))
        .collect();
    let measured = Pair::measure_all(kept, |texts| {
        parallel::map_in_order(texts, parallel::threads(), SideText::measure)
    });
    let mut inputs = RunInputs {
        config,
        index,
        reported,
    };
    let mut rows = Vec::new();
    let mut observed = Observed::default();
    for check in selection.checks() {
        let kept: Vec<&Pair> = measured
            .iter()
            .filter(|pair| check.keeps(pair.path()))
            .collect();
        if let Some(outcome) = check.run(&kept, &mut inputs)? {
            rows.extend(outcome.findings);
            observed = observed.merged(outcome.observed);
        }
    }
    Ok(Judged { rows, observed })
}

// The budget starts before the walk, so the walk and the reads count against it. The walk
// warnings come with every outcome, a refused walk and a failed index included.
pub(crate) fn built_index(
    services: &Services,
    root: &Path,
    lookup: &mut Lookup<'_>,
    measured: &[&Pair],
) -> Noted<Result<IndexedFiles, Error>> {
    let started = Instant::now();
    let walk = services.git.walk(root, scan::reads);
    let mut warnings: Vec<String> = walk.warnings.iter().map(ToString::to_string).collect();
    let files = match walk.files {
        Ok(files) => files,
        Err(refusal) => {
            let unlisted = IndexedFiles {
                unlisted: Some(refusal.to_string()),
                ..IndexedFiles::default()
            };
            return Noted {
                value: Ok(unlisted),
                diagnostics: warnings,
            };
        }
    };
    let (texts, unread) = scope_texts(root, &files);
    let budget = services.walk_budget;
    let pairs: HashMap<&str, &Pair> = measured.iter().map(|pair| (pair.path(), *pair)).collect();
    let blocks_of = |path: &str, text: &str, language: Language| {
        let stored = lookup(path, text, language);
        stored_blocks(stored, &mut warnings, || {
            measured_blocks(pairs.get(path).copied(), text, language)
        })
    };
    let indexed = checks::clone_index(&texts, &unread, blocks_of, || started.elapsed() < budget);
    Noted {
        value: indexed.map_err(Error::from),
        diagnostics: warnings,
    }
}

type Texts = BTreeMap<String, String>;

// The text of each file in clone scope, and the skip reason of each one that did not read.
fn scope_texts(root: &Path, files: &[git::RepoFile]) -> (Texts, Texts) {
    let mut texts = BTreeMap::new();
    let mut unread = BTreeMap::new();
    for file in files.iter().filter(|file| in_metric_scope(&file.path)) {
        match decoded(bounded_read(&root.join(&file.path))) {
            Ok(text) => texts.insert(file.path.clone(), text),
            Err(reason) => unread.insert(file.path.clone(), reason),
        };
    }
    (texts, unread)
}

fn stored_blocks(
    stored: Stored,
    warnings: &mut Vec<String>,
    measure: impl FnOnce() -> Result<Vec<Block>, SideError>,
) -> Result<Vec<Block>, SideError> {
    match stored {
        Stored::Hit(blocks) => return Ok(blocks),
        Stored::Unmeasured(reason) => return Err(SideError::Recorded(reason)),
        Stored::Unreadable(warning) => warnings.push(warning),
        Stored::Miss => {}
    }
    measure()
}

// A side of the run with the same path and text already holds the measure, so the text is not
// measured twice.
fn measured_blocks(
    pair: Option<&Pair>,
    text: &str,
    language: Language,
) -> Result<Vec<Block>, SideError> {
    pair.and_then(|pair| pair.measured_blocks(text))
        .unwrap_or_else(|| checks::measure_blocks(language, text))
}

#[cfg(test)]
mod tests;
