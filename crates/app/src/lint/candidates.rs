//! The candidate form of lint: candidate patches judged against the repository as it stands, by
//! the patch checks and the pair checks.
//!
//! A candidate whose patch did not parse, holds no edits, names a path outside the root, or whose
//! pre-image is not in the tree is reported with coverage rows and is not checked. Every other
//! candidate of the run is still checked.

mod admission;
mod inputs;
mod manifests;
mod selection;
mod tree;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

use checks::{CloneIndex, Pair, PairFinding, Severity};
use graph::{FileFacts, NameIndex, Resolver, Scopes};
use patch::{DiffNote, Edit, ParseResult};
use patchcheck::{CHECK_NEAR_DUPLICATES, RepoFacts};

use super::{BlockSource, LintReport, candidate, empty, with_rows};
use crate::pairs::{IndexOf, Judged, Reported, built_index};
use crate::scan::RepoFacts as Scan;
use crate::{Error, Noted, Reading, Services, Session};
use admission::{checkable, oriented, unread_row};
use manifests::read_declared_dependencies;
use tree::{lenient_texts, pair_paths, pair_rows, scanned_paths, working_reads};

pub use inputs::{load_lint_candidates, load_lint_diff};
pub use selection::CheckSelection;

/// One candidate patch: its id, its parsed edits, and the diff sections with nothing to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LintCandidateInput {
    /// The id the report names the candidate by.
    pub id: String,
    /// The edits, and each block that did not parse.
    pub parsed: ParseResult,
    /// The sections of a diff that parsed but hold no text to check.
    pub notes: Vec<DiffNote>,
}

/// One candidate lint call: the candidates, in report order, and the checks to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidatesRequest {
    /// The candidates.
    pub candidates: Vec<LintCandidateInput>,
    /// The checks.
    pub checks: CheckSelection,
}

/// Lints every candidate of `request` against `session`'s repository. A patch check makes the
/// run scan the repository once; `clone_delta` builds one clone index, with the blocks the tag
/// cache stores, that every candidate shares. The warnings are the scan's, then the index's.
#[must_use]
pub fn lint_candidates(
    services: &Services,
    session: &Session,
    request: &CandidatesRequest,
) -> Noted<Result<LintReport, Error>> {
    let tags = &session.tags;
    let mut lookup = |path: &str, text: &str, language| tags.stored(path, text, language);
    let source = BlockSource {
        cached: tags.cached(),
        lookup: &mut lookup,
    };
    candidates_with(services, &Reading::of(session), source, request)
}

// The candidate form reads the index from the tag cache when it has one, else measures it.
pub(crate) fn candidates_with(
    services: &Services,
    reading: &Reading<'_>,
    source: BlockSource<'_>,
    request: &CandidatesRequest,
) -> Noted<Result<LintReport, Error>> {
    let warnings = RefCell::new(Vec::new());
    let BlockSource { lookup, .. } = source;
    let root = &reading.repo.root;
    let mut shared: Option<Rc<CloneIndex>> = None;
    let mut index = |measured: &[&Pair]| -> Result<Rc<CloneIndex>, Error> {
        if let Some(index) = &shared {
            return Ok(Rc::clone(index));
        }
        let built = built_index(services, root, lookup, measured);
        warnings.borrow_mut().extend(built.diagnostics);
        let index = Rc::new(CloneIndex::Built(built.value?));
        shared = Some(Rc::clone(&index));
        Ok(index)
    };
    let mut judge = Judge {
        root,
        config: &reading.repo.config,
        checks: &request.checks,
        index: &mut index,
    };
    let report = if request.checks.patch_checks() {
        let scan = Scan::load(services, reading);
        warnings.borrow_mut().extend(scan.diagnostics);
        scan.value
            .and_then(|scan| with_repo(&mut judge, &scan, &request.candidates))
    } else {
        judged_all(&request.candidates, |candidate| judge.pairs_only(candidate))
    };
    Noted {
        value: report,
        diagnostics: warnings.into_inner(),
    }
}

// The pre-patch repository that every patch check compares against: the scan, the resolver over
// it, the text of each scanned file, and the declared dependencies.
fn with_repo(
    judge: &mut Judge<'_, '_>,
    scan: &Scan,
    candidates: &[LintCandidateInput],
) -> Result<LintReport, Error> {
    let names = NameIndex::build(&scan.files);
    let scopes = Scopes::build(&scan.files);
    let resolver = Resolver::new(&names, &scopes);
    let texts = scanned_texts(judge.root, &scan.files);
    let facts = RepoFacts::new(&resolver, &texts, read_declared_dependencies(judge.root));
    judged_all(candidates, |candidate| {
        judge.with_patches(candidate, &facts, &texts)
    })
}

fn judged_all(
    candidates: &[LintCandidateInput],
    mut judged: impl FnMut(&LintCandidateInput) -> Result<Judged, Error>,
) -> Result<LintReport, Error> {
    let candidates = candidates
        .iter()
        .map(|input| Ok(candidate(&input.id, judged(input)?)))
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(LintReport { candidates })
}

// The bounded read of each scanned file, decoded with U+FFFD; a file that no longer reads has no
// text, so near_duplicates reports it as unread.
fn scanned_texts(root: &Path, files: &[FileFacts]) -> BTreeMap<String, String> {
    files
        .iter()
        .filter_map(|file| {
            let bytes = io::read_bounded(&root.join(&file.path)).ok()?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            Some((file.path.clone(), text))
        })
        .collect()
}

// What every candidate of one run is judged with.
struct Judge<'a, 'i> {
    root: &'a Path,
    config: &'a io::ProjectConfig,
    checks: &'a CheckSelection,
    index: &'a mut IndexOf<'i>,
}

impl Judge<'_, '_> {
    // The patch rows come first: the admission rows, the repository warnings, the patch checks'
    // findings; then the pair rows of the files the edits change.
    fn with_patches(
        &mut self,
        candidate: &LintCandidateInput,
        facts: &RepoFacts<'_>,
        texts: &BTreeMap<String, String>,
    ) -> Result<Judged, Error> {
        let admitted = oriented(checkable(candidate, self.root), texts);
        if admitted.edits.is_empty() {
            return Ok(with_rows(admitted.rows, empty()));
        }
        let report = patchcheck::lint_patch(&admitted.edits, facts);
        let mut rows = admitted.rows;
        rows.extend(report.warnings.iter().map(|warning| unread_row(warning)));
        let kept: Vec<PairFinding> = report
            .findings
            .into_iter()
            .filter(|finding| self.checks.keeps(finding.check))
            .map(|finding| PairFinding {
                finding,
                detail: None,
            })
            .collect();
        let reported = near_duplicate_sites(&kept);
        rows.extend(kept);
        let paired = self.tree_pairs(&admitted.edits, &reported)?;
        Ok(with_rows(rows, paired))
    }

    // Without a patch check there is no scan, so orientation reads the files the scan would.
    fn pairs_only(&mut self, candidate: &LintCandidateInput) -> Result<Judged, Error> {
        let admission = checkable(candidate, self.root);
        let mut reads = working_reads(self.root, &scanned_paths(&admission.edits));
        let admitted = oriented(admission, &lenient_texts(&reads));
        if admitted.edits.is_empty() {
            return Ok(with_rows(admitted.rows, empty()));
        }
        let Some(selection) = self.checks.pairs() else {
            return Ok(with_rows(admitted.rows, empty()));
        };
        let run = tree::Run {
            index: &mut *self.index,
            reported: &Reported::new(),
        };
        let paired = pair_rows(&admitted.edits, &mut reads, selection, self.config, run)?;
        Ok(with_rows(admitted.rows, paired))
    }

    fn tree_pairs(&mut self, edits: &[Edit], reported: &Reported) -> Result<Judged, Error> {
        let Some(selection) = self.checks.pairs() else {
            return Ok(empty());
        };
        let mut reads = working_reads(self.root, &pair_paths(edits));
        let run = tree::Run {
            index: &mut *self.index,
            reported,
        };
        pair_rows(edits, &mut reads, selection, self.config, run)
    }
}

// The functions near_duplicates reported, so clone_delta does not report them again.
fn near_duplicate_sites(rows: &[PairFinding]) -> Reported {
    rows.iter()
        .map(|row| &row.finding)
        .filter(|finding| {
            finding.check == CHECK_NEAR_DUPLICATES && finding.severity == Severity::Advisory
        })
        .map(|finding| (finding.path.clone(), finding.line))
        .collect()
}

#[cfg(test)]
mod tests;
