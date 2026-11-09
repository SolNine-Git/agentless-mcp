use std::collections::BTreeMap;

pub fn combine_tallies(first: &BTreeMap<String, i64>, second: &BTreeMap<String, i64>) -> BTreeMap<String, i64> {
    let mut total = first.clone();
    for (name, amount) in second {
        *total.entry(name.clone()).or_insert(0) += amount;
    }
    total.retain(|_, amount| *amount > 0);
    total
}
