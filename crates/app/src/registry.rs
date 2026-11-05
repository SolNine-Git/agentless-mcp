//! The lint check registry, the pair-check table and selection, and the refusals of a selection.

use checks::{
    CALLEE_OVERLAP, CLONE_DELTA, COMPLEXITY_DELTA, CONTRACT_DRIFT, CYCLE_DODGE, Outcome, Pair,
};
use patchcheck::{
    CHECK_ARITY, CHECK_CYCLE_DELTA, CHECK_DANGLING_CALLERS, CHECK_DANGLING_REFERENCES,
    CHECK_NEAR_DUPLICATES, CHECK_SHADOWING, CHECK_UNDECLARED_IMPORTS,
};
use prompts::messages::{
    CALLEE_OVERLAP_READS, CYCLE_DODGE_READS, LINT_NO_CHECKS, LINT_PATH_TAKES_PAIR_CHECKS,
    LINT_SINCE_TAKES_PAIR_CHECKS, LINT_TREE_CHECK_SINCE_ONLY, LINT_UNKNOWN_CHECK,
};

use crate::callees::in_callee_scope;
use crate::dodges::in_dodge_scope;
use crate::pair_runs::{self, Run, RunInputs};
use crate::{Error, in_metric_scope, is_fixture_path, is_test_path};

/// The input that a lint check reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckInput {
    /// A candidate patch.
    Patch,
    /// The old and new text of each edited file.
    Pairs,
    /// The edited file texts and the clone index of the repository.
    PairsClones,
    /// The edited file texts and the symbol graph of the working tree.
    PairsGraph,
}

impl CheckInput {
    /// Returns the input as the capabilities report spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        ["patch", "pairs", "pairs+clones", "pairs+graph"][self as usize]
    }
}

/// One lint check and the input it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LintCheck {
    /// The check's name.
    pub name: &'static str,
    /// The input the check reads.
    pub input: CheckInput,
}

const fn check(name: &'static str, input: CheckInput) -> LintCheck {
    LintCheck { name, input }
}

/// Every lint check in registry order, which is also the order the checks run in.
pub const CHECKS: [LintCheck; 12] = [
    check(CHECK_UNDECLARED_IMPORTS, CheckInput::Patch),
    check(CHECK_SHADOWING, CheckInput::Patch),
    check(CHECK_NEAR_DUPLICATES, CheckInput::Patch),
    check(CHECK_DANGLING_REFERENCES, CheckInput::Patch),
    check(CHECK_DANGLING_CALLERS, CheckInput::Patch),
    check(CHECK_ARITY, CheckInput::Patch),
    check(CHECK_CYCLE_DELTA, CheckInput::Patch),
    PairCheck::ComplexityDelta.lint_check(),
    PairCheck::ContractDrift.lint_check(),
    PairCheck::CloneDelta.lint_check(),
    PairCheck::CalleeOverlap.lint_check(),
    PairCheck::CycleDodge.lint_check(),
];

/// A check that compares the old and new text of each edited file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PairCheck {
    /// `complexity_delta`.
    ComplexityDelta,
    /// `contract_drift`.
    ContractDrift,
    /// `clone_delta`.
    CloneDelta,
    /// `callee_overlap`.
    CalleeOverlap,
    /// `cycle_dodge`.
    CycleDodge,
}

// One row of the pair table. A check reads only the pairs whose path `keeps` accepts. Only a tree
// check has `reads`: what it reads in the working tree, which its refusal in another form names.
#[derive(Clone, Copy)]
struct PairRow {
    name: &'static str,
    input: CheckInput,
    keeps: fn(&str) -> bool,
    forms: &'static [FormKind],
    reads: Option<&'static str>,
    run: Run,
}

const EVERY_FORM: &[FormKind] = &[FormKind::Path, FormKind::Since];

// Row i is the row of the check whose discriminant is i. Fixtures are skipped by every check, and
// tests by all but complexity_delta.
const PAIR_CHECKS: [PairRow; PairCheck::ALL.len()] = [
    PairRow {
        name: COMPLEXITY_DELTA,
        input: CheckInput::Pairs,
        keeps: |path| !is_fixture_path(path),
        forms: EVERY_FORM,
        reads: None,
        run: pair_runs::complexity_delta,
    },
    PairRow {
        name: CONTRACT_DRIFT,
        input: CheckInput::Pairs,
        keeps: |path| !is_test_path(path) && !is_fixture_path(path),
        forms: EVERY_FORM,
        reads: None,
        run: pair_runs::contract_drift,
    },
    PairRow {
        name: CLONE_DELTA,
        input: CheckInput::PairsClones,
        keeps: in_metric_scope,
        forms: EVERY_FORM,
        reads: None,
        run: pair_runs::clone_delta,
    },
    PairRow {
        name: CALLEE_OVERLAP,
        input: CheckInput::PairsGraph,
        keeps: in_callee_scope,
        forms: &[FormKind::Since],
        reads: Some(CALLEE_OVERLAP_READS),
        run: pair_runs::callee_overlap,
    },
    PairRow {
        name: CYCLE_DODGE,
        input: CheckInput::PairsGraph,
        keeps: in_dodge_scope,
        forms: &[FormKind::Since],
        reads: Some(CYCLE_DODGE_READS),
        run: pair_runs::cycle_dodge,
    },
];

impl PairCheck {
    /// Every pair check, in registry order.
    pub const ALL: [PairCheck; 5] = [
        Self::ComplexityDelta,
        Self::ContractDrift,
        Self::CloneDelta,
        Self::CalleeOverlap,
        Self::CycleDodge,
    ];

    /// Returns the check's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.row().name
    }

    /// Returns the pair check named `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<PairCheck> {
        Self::ALL.into_iter().find(|check| check.name() == name)
    }

    pub(crate) fn keeps(self, path: &str) -> bool {
        (self.row().keeps)(path)
    }

    pub(crate) fn run(
        self,
        pairs: &[&Pair],
        inputs: &mut RunInputs<'_, '_>,
    ) -> Result<Option<Outcome>, Error> {
        (self.row().run)(pairs, inputs)
    }

    pub(crate) fn runs_in(self, form: FormKind) -> bool {
        self.row().forms.contains(&form)
    }

    // The check's own refusal in a form that does not run it.
    fn refused_in(self, form: FormKind) -> Option<String> {
        let row = self.row();
        let reads = row.reads.filter(|_| !self.runs_in(form))?;
        Some(LINT_TREE_CHECK_SINCE_ONLY.render((row.name, reads)))
    }

    const fn lint_check(self) -> LintCheck {
        let row = self.row();
        check(row.name, row.input)
    }

    const fn row(self) -> PairRow {
        PAIR_CHECKS[self as usize]
    }
}

/// The lint form that a selection is for. It picks the refusal text for a patch check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    /// `--path` with `--after`.
    Path,
    /// `--since`.
    Since,
}

/// A non-empty set of pair checks. The checks run in registry order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairSelection {
    chosen: [bool; PAIR_CHECKS.len()],
}

impl PairSelection {
    /// Returns the default selection: the checks that read only the edited files.
    #[must_use]
    pub fn edited_files() -> PairSelection {
        PairSelection {
            chosen: PAIR_CHECKS.map(|row| row.input == CheckInput::Pairs),
        }
    }

    /// Returns the selection of `checks`.
    ///
    /// # Errors
    ///
    /// Returns the refusal text when `checks` is empty.
    pub fn of(checks: &[PairCheck]) -> Result<PairSelection, String> {
        let mut chosen = [false; PAIR_CHECKS.len()];
        for check in checks {
            chosen[*check as usize] = true;
        }
        if chosen.contains(&true) {
            Ok(PairSelection { chosen })
        } else {
            Err(no_checks())
        }
    }

    /// Returns the selection of the check `names` for the lint `form`.
    ///
    /// # Errors
    ///
    /// Returns the refusal text for an empty selection, an unknown name, or a check that is not a
    /// pair check.
    pub fn from_names(names: &[String], form: FormKind) -> Result<PairSelection, String> {
        if let Some(refusal) = unknown_check_refusal(names) {
            return Err(refusal);
        }
        if let Some(refusal) = form_refusal(names, form) {
            return Err(refusal);
        }
        if let Some(refusal) = pair_only_refusal(names, form) {
            return Err(refusal);
        }
        let checks: Vec<PairCheck> = names
            .iter()
            .filter_map(|name| PairCheck::from_name(name))
            .collect();
        PairSelection::of(&checks)
    }

    /// Returns the chosen checks in registry order.
    pub fn checks(self) -> impl Iterator<Item = PairCheck> {
        PairCheck::ALL
            .into_iter()
            .filter(move |check| self.chosen[*check as usize])
    }
}

/// Returns why `names` is not a usable check selection, or `None` when every name is a check.
#[must_use]
pub fn unknown_check_refusal(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return Some(no_checks());
    }
    let unknown = names
        .iter()
        .find(|name| !CHECKS.iter().any(|check| check.name == name.as_str()))?;
    Some(LINT_UNKNOWN_CHECK.render((unknown, &all_names())))
}

// The own refusal of the first pair check in `names` that `form` does not run.
pub(crate) fn form_refusal(names: &[String], form: FormKind) -> Option<String> {
    names
        .iter()
        .find_map(|name| PairCheck::from_name(name)?.refused_in(form))
}

// The names that a pair form cannot run, in input order, as the form's refusal.
fn pair_only_refusal(names: &[String], form: FormKind) -> Option<String> {
    let refused: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| PairCheck::from_name(name).is_none_or(|check| !check.runs_in(form)))
        .collect();
    if refused.is_empty() {
        return None;
    }
    let template = match form {
        FormKind::Path => LINT_PATH_TAKES_PAIR_CHECKS,
        FormKind::Since => LINT_SINCE_TAKES_PAIR_CHECKS,
    };
    let runs: Vec<&str> = PairCheck::ALL
        .into_iter()
        .filter(|check| check.runs_in(form))
        .map(PairCheck::name)
        .collect();
    Some(template.render((&runs.join(", "), &refused.join(", "))))
}

fn no_checks() -> String {
    LINT_NO_CHECKS.render(&all_names())
}

fn all_names() -> String {
    CHECKS.map(|check| check.name).join(", ")
}

#[cfg(test)]
mod tests;
