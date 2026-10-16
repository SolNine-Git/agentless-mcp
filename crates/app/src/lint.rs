//! The lint operation: the proposed, revision and candidate forms, with their report.

mod candidates;

use std::cell::RefCell;
use std::rc::Rc;

use checks::{CloneIndex, Observed, PairFinding};
use serde::Serialize;

use crate::pairs::{IndexOf, Judged, Lookup, Reported, built_index, run_pairs};
use crate::proposed::{After, Proposed, proposed};
use crate::revision::compared;
use crate::{Error, Noted, PairSelection, Repo, Services, Tags};

pub use candidates::{
    CandidatesRequest, CheckSelection, LintCandidateInput, lint_candidates, load_lint_candidates,
    load_lint_diff,
};

/// Runs the pair checks of `request` on `repo`, with the clone blocks that `tags` stores. The
/// error is a revision that names no commit, git output that does not parse, or a check that
/// fails on input it cannot read. The warnings come with either outcome.
pub fn lint(
    services: &Services,
    repo: &Repo,
    tags: &Tags,
    request: &LintRequest,
) -> Noted<Result<LintReport, Error>> {
    let mut lookup = |path: &str, text: &str, language| tags.stored(path, text, language);
    let source = BlockSource {
        cached: tags.cached(),
        lookup: &mut lookup,
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

// The tag cache as the lint run reads it: whether one is usable, and its stored blocks.
pub(crate) struct BlockSource<'a> {
    pub(crate) cached: bool,
    pub(crate) lookup: &'a mut Lookup<'a>,
}

// The clone index walk raises the warnings, so they reach the caller with either outcome.
pub(crate) fn lint_with(
    services: &Services,
    repo: &Repo,
    source: BlockSource<'_>,
    request: &LintRequest,
) -> Noted<Result<LintReport, Error>> {
    let warnings = RefCell::new(Vec::new());
    let BlockSource { cached, lookup } = source;
    let mut index = |measured: &[&checks::Pair]| -> Result<Rc<CloneIndex>, Error> {
        let built = built_index(services, &repo.root, lookup, measured);
        warnings.borrow_mut().extend(built.diagnostics);
        built.value.map(|files| Rc::new(CloneIndex::Built(files)))
    };
    let report = report(services, repo, cached, request, &mut index);
    Noted {
        value: report,
        diagnostics: warnings.into_inner(),
    }
}

// Without a usable tag cache, the proposed form reports the no_tag_cache gap and reads no other
// file. The revision form measures each file in memory instead.
fn report(
    services: &Services,
    repo: &Repo,
    cached: bool,
    request: &LintRequest,
    index: &mut IndexOf<'_>,
) -> Result<LintReport, Error> {
    let (id, judged) = match &request.form {
        LintForm::Proposed { path, after } => {
            let compared = proposed(&repo.root, path, after, services.cwd.as_deref());
            let mut no_cache = |_: &[&checks::Pair]| Ok(Rc::new(CloneIndex::NoTagCache));
            let index: &mut IndexOf<'_> = if cached { index } else { &mut no_cache };
            (
                path,
                judge_proposed(compared, request.checks, &repo.config, index)?,
            )
        }
        LintForm::Revision { revision } => {
            let compared = compared(&services.git, &repo.root, revision)?;
            let none = Reported::new();
            let judged = run_pairs(compared.pairs, request.checks, &repo.config, index, &none)?;
            (revision, with_rows(compared.rows, judged))
        }
    };
    Ok(LintReport {
        candidates: vec![candidate(id, judged)],
    })
}

fn judge_proposed(
    compared: Proposed,
    checks: PairSelection,
    config: &io::ProjectConfig,
    index: &mut IndexOf<'_>,
) -> Result<Judged, Error> {
    match compared {
        Proposed::Pair(pair) => run_pairs(vec![pair], checks, config, index, &Reported::new()),
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
