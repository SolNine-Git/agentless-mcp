use crate::helpers::{rk_label, rk_score, rk_sort, rk_trim};


pub fn order_items(items: &[i64]) -> Vec<i64> {
    let limit = rk_trim(rk_sort(items.len() as i64));
    items
        .iter()
        .map(|item| rk_score(*item))
        .take(limit as usize)
        .map(|score| rk_label(score) + 1)
        .collect()
}
