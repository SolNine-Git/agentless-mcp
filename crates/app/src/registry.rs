//! The lint check registry, the pair-check selection, and the refusals of a selection.

use prompts::messages::{
    LINT_NO_CHECKS, LINT_PATH_TAKES_PAIR_CHECKS, LINT_SINCE_TAKES_PAIR_CHECKS, LINT_UNKNOWN_CHECK,
};

/// The input that a lint check reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckInput {
    /// A candidate patch.
    Patch,
    /// The old and new text of each edited file.
    Pairs,
    /// The edited file texts and the clone index of the repository.
    PairsClones,
}

impl CheckInput {
    /// Returns the input as the capabilities report spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        ["patch", "pairs", "pairs+clones"][self as usize]
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
pub const CHECKS: [LintCheck; 10] = [
    check("undeclared_imports", CheckInput::Patch),
    check("shadowing", CheckInput::Patch),
    check("near_duplicates", CheckInput::Patch),
    check("dangling_references", CheckInput::Patch),
    check("dangling_callers", CheckInput::Patch),
    check("arity", CheckInput::Patch),
    check("cycle_delta", CheckInput::Patch),
    check(PairCheck::ComplexityDelta.name(), CheckInput::Pairs),
    check(PairCheck::ContractDrift.name(), CheckInput::Pairs),
    check(PairCheck::CloneDelta.name(), CheckInput::PairsClones),
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
}

impl PairCheck {
    /// Every pair check, in registry order.
    pub const ALL: [PairCheck; 3] = [Self::ComplexityDelta, Self::ContractDrift, Self::CloneDelta];

    /// Returns the check's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ComplexityDelta => "complexity_delta",
            Self::ContractDrift => "contract_drift",
            Self::CloneDelta => "clone_delta",
        }
    }

    /// Returns the pair check named `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<PairCheck> {
        Self::ALL.into_iter().find(|check| check.name() == name)
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
    chosen: [bool; 3],
}

impl PairSelection {
    /// Returns the default selection: the checks that read only the edited files.
    #[must_use]
    pub fn edited_files() -> PairSelection {
        PairSelection {
            chosen: [true, true, false],
        }
    }

    /// Returns the selection of `checks`.
    ///
    /// # Errors
    ///
    /// Returns the refusal text when `checks` is empty.
    pub fn of(checks: &[PairCheck]) -> Result<PairSelection, String> {
        let mut chosen = [false; 3];
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

    /// Returns true when the selection holds `check`.
    #[must_use]
    pub fn contains(self, check: PairCheck) -> bool {
        self.chosen[check as usize]
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

// The names that a pair form cannot run, in input order, as the form's refusal.
fn pair_only_refusal(names: &[String], form: FormKind) -> Option<String> {
    let patch: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| PairCheck::from_name(name).is_none())
        .collect();
    if patch.is_empty() {
        return None;
    }
    let template = match form {
        FormKind::Path => LINT_PATH_TAKES_PAIR_CHECKS,
        FormKind::Since => LINT_SINCE_TAKES_PAIR_CHECKS,
    };
    let pair_checks = PairCheck::ALL.map(PairCheck::name).join(", ");
    Some(template.render((&pair_checks, &patch.join(", "))))
}

fn no_checks() -> String {
    LINT_NO_CHECKS.render(&all_names())
}

fn all_names() -> String {
    CHECKS.map(|check| check.name).join(", ")
}

#[cfg(test)]
mod tests;
