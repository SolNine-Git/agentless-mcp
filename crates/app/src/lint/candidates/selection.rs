//! The checks of a candidate run: the patch checks apart from the pair checks.

use crate::registry::form_refusal;
use crate::{CHECKS, CheckInput, FormKind, PairCheck, PairSelection, unknown_check_refusal};

// A candidate run reads new text from memory against the working tree, as the --path form does,
// so it runs the pair checks of that form.
const FORM: FormKind = FormKind::Path;

/// The checks of a candidate run: the patch checks by name and the pair checks, each in registry
/// order. It holds at least one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckSelection {
    patch: Vec<&'static str>,
    pairs: Option<PairSelection>,
}

impl CheckSelection {
    /// Returns every check but the opt-in pair checks.
    #[must_use]
    pub fn all() -> CheckSelection {
        let opt_in = |name: &str| PairCheck::from_name(name).is_some_and(PairCheck::opt_in);
        Self::chosen(|name| !opt_in(name))
    }

    /// Returns the checks that `names` names.
    ///
    /// # Errors
    ///
    /// Returns the refusal text for an empty selection, a name that is no check, or a pair check
    /// that a candidate run does not run.
    pub fn from_names(names: &[String]) -> Result<CheckSelection, String> {
        if let Some(refusal) = unknown_check_refusal(names) {
            return Err(refusal);
        }
        if let Some(refusal) = form_refusal(names, FORM) {
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
            .filter(|check| check.runs_in(FORM) && wanted(check.name()))
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
