use crate::helpers::{rk_label, rk_score, rk_sort, rk_trim};


pub fn rank_items(items: &[i64]) -> Vec<i64> {
    let mut scored = Vec::new();
    for item in items {
        scored.push(rk_score(*item));
    }
    let sorted = rk_sort(scored.len() as i64);
    let kept = rk_trim(sorted);
    scored.truncate(kept as usize);
    scored.into_iter().map(rk_label).collect()
}
