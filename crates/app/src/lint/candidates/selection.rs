//! The checks of a candidate run: any of the ten, the patch checks apart from the pair checks.

use crate::{CHECKS, CheckInput, PairCheck, PairSelection, unknown_check_refusal};

/// The checks of a candidate run: the patch checks by name and the pair checks, each in registry
/// order. It holds at least one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckSelection {
    patch: Vec<&'static str>,
    pairs: Option<PairSelection>,
}

impl CheckSelection {
    /// Returns every check.
    #[must_use]
    pub fn all() -> CheckSelection {
        Self::chosen(|_| true)
    }

    /// Returns the checks that `names` names.
    ///
    /// # Errors
    ///
    /// Returns the refusal text for an empty selection or a name that is no check.
    pub fn from_names(names: &[String]) -> Result<CheckSelection, String> {
        if let Some(refusal) = unknown_check_refusal(names) {
            return Err(refusal);
        }
        Ok(Self::chosen(|name| names.iter().any(|given| given == name)))
    }

    fn chosen(wanted: impl Fn(&str) -> bool) -> CheckSelection {
        let patch = CHECKS
            .iter()
            .filter(|check| check.input == CheckInput::Patch && wanted(check.name))
            .map(|check| check.name)
            .collect();
        let pairs: Vec<PairCheck> = PairCheck::ALL
            .into_iter()
            .filter(|check| wanted(check.name()))
            .collect();
        CheckSelection {
            patch,
            pairs: PairSelection::of(&pairs).ok(),
        }
    }

    // A patch check makes the run scan the repository.
    pub(crate) fn patch_checks(&self) -> bool {
        !self.patch.is_empty()
    }

    // The coverage rows of the patch checks are kept whatever the selection.
    pub(crate) fn keeps(&self, check: &str) -> bool {
        check == patchcheck::CHECK_COVERAGE || self.patch.contains(&check)
    }

    pub(crate) fn pairs(&self) -> Option<PairSelection> {
        self.pairs
    }
}
