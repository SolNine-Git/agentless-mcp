//! The lint operation: the proposed form and the revision form, with their report.

use std::cell::RefCell;

use checks::{CloneIndex, Observed, PairFinding};
use serde::Serialize;

use crate::pairs::{IndexOf, Judged, Lookup, built_index, run_pairs};
use crate::proposed::{After, Proposed, proposed};
use crate::revision::compared;
use crate::{Error, PairSelection, Repo, Services, Tags};

/// Runs the pair checks of `request` on `repo`, with the clone blocks that `tags` stores.
///
/// # Errors
///
/// Returns a revision that names no commit, git output that does not parse, or a check that
/// fails on input it cannot read.
pub fn lint(
    services: &Services,
    repo: &Repo,
    tags: &Tags,
    request: &LintRequest,
) -> Result<Noted<LintReport>, Error> {
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

/// A value and the warnings to print on stderr, in the order they arose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noted<T> {
    /// The value.
    pub value: T,
    /// The warnings.
    pub diagnostics: Vec<String>,
}

// The tag cache as the lint run reads it: whether one is usable, and its stored blocks.
pub(crate) struct BlockSource<'a> {
    pub(crate) cached: bool,
    pub(crate) lookup: &'a mut Lookup<'a>,
}

// Without a usable tag cache, the proposed form reports the no_tag_cache gap and reads no other
// file. The revision form measures each file in memory instead.
pub(crate) fn lint_with(
    services: &Services,
    repo: &Repo,
    source: BlockSource<'_>,
    request: &LintRequest,
) -> Result<Noted<LintReport>, Error> {
    let warnings = RefCell::new(Vec::new());
    let BlockSource { cached, lookup } = source;
    let mut index = |measured: &[&checks::Pair]| -> Result<CloneIndex, Error> {
        let (files, found) = built_index(services, &repo.root, lookup, measured)?;
        warnings.borrow_mut().extend(found);
        Ok(CloneIndex::Built(files))
    };
    let (id, judged) = match &request.form {
        LintForm::Proposed { path, after } => {
            let compared = proposed(&repo.root, path, after, services.cwd.as_deref());
            let mut no_cache = |_: &[&checks::Pair]| Ok(CloneIndex::NoTagCache);
            let index: &mut IndexOf<'_> = if cached { &mut index } else { &mut no_cache };
            (
                path,
                judge_proposed(compared, request.checks, &repo.config, index)?,
            )
        }
        LintForm::Revision { revision } => {
            let compared = compared(&services.git, &repo.root, revision)?;
            let judged = run_pairs(compared.pairs, request.checks, &repo.config, &mut index)?;
            (revision, with_rows(compared.rows, judged))
        }
    };
    let candidate = candidate(id, judged);
    Ok(Noted {
        value: LintReport {
            candidates: vec![candidate],
        },
        diagnostics: warnings.into_inner(),
    })
}

fn judge_proposed(
    compared: Proposed,
    checks: PairSelection,
    config: &io::ProjectConfig,
    index: &mut IndexOf<'_>,
) -> Result<Judged, Error> {
    match compared {
        Proposed::Pair(pair) => run_pairs(vec![pair], checks, config, index),
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
