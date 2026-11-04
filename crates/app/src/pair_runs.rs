//! The run of each pair check on the pairs it keeps, and what a run reads besides the pairs.

use std::collections::BTreeSet;
use std::rc::Rc;

use checks::{CalleeTree, CloneIndex, Outcome, Pair, Side};

use crate::{Error, parallel};

// The clone index of a run, given the measured pairs that clone_delta keeps. Every candidate of
// a run shares one index.
pub(crate) type IndexOf<'a> = dyn FnMut(&[&Pair]) -> Result<Rc<CloneIndex>, Error> + 'a;

// The path and line of each function that near_duplicates reported in the same run.
pub(crate) type Reported = BTreeSet<(String, u32)>;

// The callee sets of the working tree, which only the --since form has.
pub(crate) type CalleesOf<'a> = dyn FnMut() -> Result<CalleeTree, Error> + 'a;

// What a run reads besides its pairs.
pub(crate) struct RunInputs<'r, 'i> {
    pub(crate) config: &'r io::ProjectConfig,
    pub(crate) index: &'r mut IndexOf<'i>,
    pub(crate) reported: &'r Reported,
    pub(crate) callees: Option<&'r mut CalleesOf<'i>>,
}

// A check's run on the pairs it keeps. `None` is a run that gives no outcome at all.
pub(crate) type Run = fn(&[&Pair], &mut RunInputs<'_, '_>) -> Result<Option<Outcome>, Error>;

pub(crate) fn complexity_delta(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    let threshold = inputs
        .config
        .complexity_threshold
        .unwrap_or(metrics::DEFAULT_THRESHOLD);
    Ok(Some(checks::complexity_delta(pairs, threshold)?))
}

pub(crate) fn contract_drift(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    let globs = &inputs.config.boundary_globs;
    Ok(Some(checks::contract_drift(pairs, globs)?))
}

// With no pair kept there is nothing to compare, so the run builds no index and gives no outcome.
pub(crate) fn clone_delta(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    if pairs.is_empty() {
        return Ok(None);
    }
    find_blocks(pairs);
    let index: Rc<CloneIndex> = (inputs.index)(pairs)?;
    Ok(Some(checks::clone_delta(pairs, &index, inputs.reported)?))
}

// The selection refuses callee_overlap in every form that has no tree, so the tree is there.
pub(crate) fn callee_overlap(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let callees = inputs.callees.as_mut();
    let tree = (callees.expect("only the --since form selects callee_overlap"))()?;
    Ok(Some(checks::callee_overlap(pairs, &tree)?))
}

// Only clone_delta reads blocks, so the pool finds them before the index and the check read them.
fn find_blocks(pairs: &[&Pair]) {
    let sides: Vec<&Side> = pairs.iter().flat_map(|pair| pair.sides()).collect();
    parallel::map_in_order(&sides, parallel::threads(), |side| side.find_blocks());
}
