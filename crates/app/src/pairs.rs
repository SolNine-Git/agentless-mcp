//! The pair run: the path filter of each check, one measure per pair, and the clone index.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use checks::{CloneIndex, IndexedFiles, Observed, Outcome, Pair, PairFinding, SideError, TextPair};
use metrics::Block;
use syntax::Language;

use crate::rows::bounded_read;
use crate::{
    Error, PairCheck, PairSelection, Services, in_metric_scope, is_fixture_path, is_test_path,
};

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
    index: &mut dyn FnMut() -> Result<CloneIndex, Error>,
) -> Result<Judged, Error> {
    let measured: Vec<Pair> = pairs
        .into_iter()
        .filter(|pair| selection.checks().any(|check| keeps(check, &pair.path)))
        .map(Pair::measure)
        .collect();
    let mut rows = Vec::new();
    let mut observed = Observed::default();
    for check in selection.checks() {
        let kept: Vec<&Pair> = measured
            .iter()
            .filter(|pair| keeps(check, pair.path()))
            .collect();
        if let Some(outcome) = run_check(check, &kept, config, index)? {
            rows.extend(outcome.findings);
            observed = observed.merged(outcome.observed);
        }
    }
    Ok(Judged { rows, observed })
}

// Fixtures are skipped by every check, and tests by all but complexity_delta.
fn keeps(check: PairCheck, path: &str) -> bool {
    match check {
        PairCheck::ComplexityDelta => !is_fixture_path(path),
        PairCheck::ContractDrift => !is_test_path(path) && !is_fixture_path(path),
        PairCheck::CloneDelta => in_metric_scope(path),
    }
}

fn run_check(
    check: PairCheck,
    pairs: &[&Pair],
    config: &io::ProjectConfig,
    index: &mut dyn FnMut() -> Result<CloneIndex, Error>,
) -> Result<Option<Outcome>, Error> {
    let outcome = match check {
        PairCheck::ComplexityDelta => {
            let threshold = config
                .complexity_threshold
                .unwrap_or(metrics::DEFAULT_THRESHOLD);
            checks::complexity_delta(pairs, threshold)
        }
        PairCheck::ContractDrift => checks::contract_drift(pairs, &config.boundary_globs),
        PairCheck::CloneDelta if pairs.is_empty() => return Ok(None),
        PairCheck::CloneDelta => checks::clone_delta(pairs, &index()?),
    };
    Ok(Some(outcome?))
}

// The budget starts before the walk, so the walk and the reads count against it.
pub(crate) fn built_index(
    services: &Services,
    root: &Path,
    lookup: &mut Lookup<'_>,
) -> Result<(IndexedFiles, Vec<String>), Error> {
    let started = Instant::now();
    let walk = match services.git.walk(root) {
        Ok(walk) => walk,
        Err(refusal) => {
            let unlisted = IndexedFiles {
                unlisted: Some(refusal.to_string()),
                ..IndexedFiles::default()
            };
            return Ok((unlisted, Vec::new()));
        }
    };
    let mut warnings: Vec<String> = walk.warnings.iter().map(ToString::to_string).collect();
    let (texts, unread) = scope_texts(root, &walk.files);
    let budget = services.walk_budget;
    let blocks_of = |path: &str, text: &str, language: Language| {
        stored_blocks(lookup(path, text, language), text, language, &mut warnings)
    };
    let files = checks::clone_index(&texts, &unread, blocks_of, || started.elapsed() < budget)?;
    Ok((files, warnings))
}

type Texts = BTreeMap<String, String>;

// The text of each file in clone scope, and the skip reason of each one that did not read.
fn scope_texts(root: &Path, files: &[git::RepoFile]) -> (Texts, Texts) {
    let mut texts = BTreeMap::new();
    let mut unread = BTreeMap::new();
    for file in files.iter().filter(|file| in_metric_scope(&file.path)) {
        let read = bounded_read(&root.join(&file.path))
            .and_then(|bytes| io::decode_strict(bytes).map_err(|error| error.to_string()));
        match read {
            Ok(text) => texts.insert(file.path.clone(), text),
            Err(reason) => unread.insert(file.path.clone(), reason),
        };
    }
    (texts, unread)
}

fn stored_blocks(
    stored: Stored,
    text: &str,
    language: Language,
    warnings: &mut Vec<String>,
) -> Result<Vec<Block>, SideError> {
    match stored {
        Stored::Hit(blocks) => return Ok(blocks),
        Stored::Unmeasured(reason) => return Err(SideError::Recorded(reason)),
        Stored::Unreadable(warning) => warnings.push(warning),
        Stored::Miss => {}
    }
    checks::measure_text(language, text).map(|measured| measured.blocks)
}

#[cfg(test)]
mod tests;
