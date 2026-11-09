use std::collections::BTreeMap;

pub fn merge_counts(left: &BTreeMap<String, i64>, right: &BTreeMap<String, i64>) -> BTreeMap<String, i64> {
    let mut merged = left.clone();
    for (key, count) in right {
        *merged.entry(key.clone()).or_insert(0) += count;
    }
    merged.retain(|_, count| *count > 0);
    merged
}
