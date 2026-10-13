//! The max-min fair split of one token budget, which every view that cuts what it shows shares.

use std::collections::BTreeSet;

// The tokens kept back on each cut item for the marker that announces the cut.
pub(crate) const TRUNCATION_MARKER_TOKENS: usize = 32;

// The indices to cut, and the equal share each cut item gets. Each round settles the items that
// fit an equal share of what is left; the items that never fit get the last share.
pub(crate) fn allocate(costs: &[usize], budget: usize) -> (BTreeSet<usize>, usize) {
    let mut pending: BTreeSet<usize> = (0..costs.len()).collect();
    let mut remaining = budget;
    while !pending.is_empty() {
        let share = remaining / pending.len();
        let settled: Vec<usize> = pending
            .iter()
            .copied()
            .filter(|&index| costs[index] <= share)
            .collect();
        if settled.is_empty() {
            break;
        }
        for index in settled {
            remaining -= costs[index];
            pending.remove(&index);
        }
    }
    if pending.is_empty() {
        return (pending, 0);
    }
    let share = remaining / pending.len();
    (pending, share)
}

// The largest count from `low` through `high` that `fits`, or `low` when none fits. `fits` must
// hold for every count below one it holds for. A binary search keeps the counter calls few.
pub(crate) fn longest_fitting(low: usize, high: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (low, high);
    while low < high {
        let middle = (low + high).div_ceil(2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

#[cfg(test)]
mod tests;
