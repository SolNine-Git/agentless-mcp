//! The run of each pair check on the pairs it keeps, and what a run reads besides the pairs.

use std::collections::BTreeSet;
use std::rc::Rc;

use checks::{CloneIndex, Outcome, Pair, Side};

use crate::callees::callee_tree;
use crate::reuse::{self, ReuseSource};
use crate::tree_scan::TreeScan;
use crate::{Error, dodges, parallel};

// The clone index of a run, given the measured pairs that clone_delta keeps. Every candidate of
// a run shares one index.
pub(crate) type IndexOf<'a> = dyn FnMut(&[&Pair]) -> Result<Rc<CloneIndex>, Error> + 'a;

// The path and line of each function that near_duplicates reported in the same run.
pub(crate) type Reported = BTreeSet<(String, u32)>;

// The scan of the working tree, which only the --since form has. Every tree check of a run
// shares one scan.
pub(crate) type TreeOf<'a> = dyn FnMut() -> Result<&'a TreeScan, Error> + 'a;

// What a run reads besides its pairs.
pub(crate) struct RunInputs<'r, 'i> {
    pub(crate) config: &'r io::ProjectConfig,
    pub(crate) index: &'r mut IndexOf<'i>,
    pub(crate) reported: &'r Reported,
    pub(crate) tree: Option<&'r mut TreeOf<'i>>,
    pub(crate) reuse: &'r mut ReuseSource<'i>,
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

pub(crate) fn callee_overlap(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let callees = callee_tree(scanned(inputs)?)?;
    Ok(Some(checks::callee_overlap(pairs, &callees)?))
}

pub(crate) fn cycle_dodge(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    if pairs.is_empty() {
        return Ok(None);
    }
    Ok(Some(dodges::cycle_dodge(scanned(inputs)?, pairs)?))
}

// The --since form gives the tree scan, which resolves the references between a row's functions.
pub(crate) fn reuse_candidates(
    pairs: &[&Pair],
    inputs: &mut RunInputs<'_, '_>,
) -> Result<Option<Outcome>, Error> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let scan = match inputs.tree.as_mut() {
        Some(tree) => Some(tree()?),
        None => None,
    };
    Ok(Some(reuse::reuse_candidates(pairs, inputs.reuse, scan)?))
}

// The selection refuses a tree check in every form that has no tree, so the tree is there.
fn scanned<'i>(inputs: &mut RunInputs<'_, 'i>) -> Result<&'i TreeScan, Error> {
    let tree = inputs.tree.as_mut();
    (tree.expect("only the --since form selects a tree check"))()
}

// Only clone_delta reads blocks, so the pool finds them before the index and the check read them.
fn find_blocks(pairs: &[&Pair]) {
    let sides: Vec<&Side> = pairs.iter().flat_map(|pair| pair.sides()).collect();
    parallel::map_in_order(&sides, parallel::threads(), |side| side.find_blocks());
}
