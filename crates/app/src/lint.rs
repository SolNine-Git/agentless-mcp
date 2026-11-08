//! The lint operation: the proposed, revision and candidate forms, with their report.

mod candidates;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use checks::{CloneIndex, Observed, PairFinding};
use serde::Serialize;

use crate::detached::drop_detached;
use crate::pair_runs::{IndexOf, Reported, RunInputs, TreeOf};
use crate::pairs::{Judged, Lookup, built_index, run_pairs};
use crate::proposed::{After, Proposed, proposed};
use crate::reuse::{ReuseSource, UnitLookup};
use crate::revision::compared;
use crate::tree_scan::{TreeScan, tree_scan};
use crate::{Error, Noted, PairSelection, Repo, Services, Tags};

pub use candidates::{
    CandidatesRequest, CheckSelection, LintCandidateInput, lint_candidates, load_lint_candidates,
    load_lint_diff,
};
pub(crate) use candidates::{name, patch_files, stem};

/// Runs the pair checks of `request` on `repo`, with the clone blocks and units that `tags`
/// stores. The error is a revision that names no commit, git output that does not parse, or a
/// check that fails on input it cannot read. The warnings come with either outcome.
pub fn lint(
    services: &Services,
    repo: &Repo,
    tags: &Tags,
    request: &LintRequest,
) -> Noted<Result<LintReport, Error>> {
    let mut lookup = |path: &str, text: &str, language| tags.stored(path, text, language);
    let mut units = |path: &str, text: &str, language| tags.stored_units(path, text, language);
    let source = BlockSource {
        cached: tags.cached(),
        lookup: &mut lookup,
        units: &mut units,
    };
    lint_with(services, repo, source, request)
}

/// The two pair forms of lint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LintForm {
    /// One repository file, by its path below the root, and its proposed whole new text.
    Proposed {
        /// The path as the caller gave it; the report names the candidate by it.
        path: String,
        /// The proposed new text.
        after: After,
    },
    /// Each file changed since the commit `revision` names, against its working-tree text.
    Revision {
        /// The revision as the caller gave it; the report names the candidate by it.
        revision: String,
    },
}

/// One lint call: the form and the pair checks to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LintRequest {
    /// What to compare.
    pub form: LintForm,
    /// The checks to run.
    pub checks: PairSelection,
}

/// The lint report: one candidate per call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LintReport {
    /// The candidates, in input order.
    pub candidates: Vec<LintCandidate>,
}

/// Every finding of one candidate, and what the pair checks measured in its new text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LintCandidate {
    /// The path or the revision, as the caller gave it.
    pub id: String,
    /// The coverage rows first, then the findings in check order.
    pub findings: Vec<PairFinding>,
    /// What the checks measured, or `None` when no check measured a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<Observed>,
}

// The tag cache as the lint run reads it: whether one is usable, its stored blocks, and its
// stored units.
pub(crate) struct BlockSource<'a> {
    pub(crate) cached: bool,
    pub(crate) lookup: &'a mut Lookup<'a>,
    pub(crate) units: &'a mut UnitLookup<'a>,
}

// The clone index walk and the tree scan raise the warnings, so they reach the caller with
// either outcome. The scan is freed on a detached thread once the report is made.
pub(crate) fn lint_with(
    services: &Services,
    repo: &Repo,
    source: BlockSource<'_>,
    request: &LintRequest,
) -> Noted<Result<LintReport, Error>> {
    let warnings = RefCell::new(Vec::new());
    let scanned: OnceCell<TreeScan> = OnceCell::new();
    let BlockSource {
        cached,
        lookup,
        units,
    } = source;
    let mut index = |measured: &[&checks::Pair]| -> Result<Rc<CloneIndex>, Error> {
        let built = built_index(services, &repo.root, lookup, measured);
        warnings.borrow_mut().extend(built.diagnostics);
        built.value.map(|files| Rc::new(CloneIndex::Built(files)))
    };
    let mut scan = || -> Result<&TreeScan, Error> {
        if let Some(scan) = scanned.get() {
            return Ok(scan);
        }
        let built = tree_scan(services, &repo.root);
        warnings.borrow_mut().extend(built.diagnostics);
        let scan = built.value?;
        Ok(scanned.get_or_init(|| scan))
    };
    let mut reuse = ReuseSource {
        services,
        root: &repo.root,
        no_tag_cache: !cached,
        units,
        warnings: &warnings,
    };
    let tree = TreeSources {
        cached,
        index: &mut index,
        scan: &mut scan,
        reuse: &mut reuse,
    };
    let report = report(services, repo, request, tree);
    if let Some(scan) = scanned.into_inner() {
        drop_detached(scan.facts);
    }
    Noted {
        value: report,
        diagnostics: warnings.into_inner(),
    }
}

// What the run reads from the tree besides the edited files: whether a tag cache is usable, the
// clone index, the scan that the tree checks share, and the source of reuse_candidates.
struct TreeSources<'r, 'i> {
    cached: bool,
    index: &'r mut IndexOf<'i>,
    scan: &'r mut TreeOf<'i>,
    reuse: &'r mut ReuseSource<'i>,
}

// Without a usable tag cache, the proposed form reports the no_tag_cache gap and reads no other
// file. The revision form measures each file in memory instead.
fn report(
    services: &Services,
    repo: &Repo,
    request: &LintRequest,
    tree: TreeSources<'_, '_>,
) -> Result<LintReport, Error> {
    let TreeSources {
        cached,
        index,
        scan,
        reuse,
    } = tree;
    let (id, judged) = match &request.form {
        LintForm::Proposed { path, after } => {
            let compared = proposed(&repo.root, path, after, services.cwd.as_deref());
            let mut no_cache = |_: &[&checks::Pair]| Ok(Rc::new(CloneIndex::NoTagCache));
            let index: &mut IndexOf<'_> = if cached { index } else { &mut no_cache };
            (
                path,
                judge_proposed(compared, request.checks, &repo.config, index, reuse)?,
            )
        }
        LintForm::Revision { revision } => {
            let compared = compared(&services.git, &repo.root, revision)?;
            let inputs = RunInputs {
                config: &repo.config,
                index,
                reported: &Reported::new(),
                tree: Some(scan),
                reuse,
            };
            let judged = run_pairs(compared.pairs, request.checks, inputs)?;
            (revision, with_rows(compared.rows, judged))
        }
    };
    Ok(LintReport {
        candidates: vec![candidate(id, judged)],
    })
}

fn judge_proposed<'i>(
    compared: Proposed,
    checks: PairSelection,
    config: &io::ProjectConfig,
    index: &mut IndexOf<'i>,
    reuse: &mut ReuseSource<'i>,
) -> Result<Judged, Error> {
    match compared {
        Proposed::Pair(pair) => {
            let inputs = RunInputs {
                config,
                index,
                reported: &Reported::new(),
                tree: None,
                reuse,
            };
            run_pairs(vec![pair], checks, inputs)
        }
        Proposed::Rows(rows) => Ok(with_rows(rows, empty())),
        Proposed::Skipped => Ok(empty()),
    }
}

fn empty() -> Judged {
    Judged {
        rows: Vec::new(),
        observed: Observed::default(),
    }
}

fn with_rows(mut rows: Vec<PairFinding>, judged: Judged) -> Judged {
    rows.extend(judged.rows);
    Judged {
        rows,
        observed: judged.observed,
    }
}

fn candidate(id: &str, judged: Judged) -> LintCandidate {
    let observed = judged.observed;
    let measured = observed.units_over_threshold.is_some()
        || observed.units_changed.is_some()
        || observed.contract_keys.is_some()
        || observed.clone_instances.is_some();
    LintCandidate {
        id: id.to_owned(),
        findings: judged.rows,
        observed: measured.then_some(observed),
    }
}

#[cfg(test)]
mod tests;
