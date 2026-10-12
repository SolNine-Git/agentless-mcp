//! Selection with a fixed tie rule: the earliest of equal items wins.

/// Returns the earliest item with the greatest key; a later item wins only with a strictly greater key.
#[must_use]
pub fn max_first<I, K, F>(items: I, mut key: F) -> Option<I::Item>
where
    I: IntoIterator,
    K: PartialOrd,
    F: FnMut(&I::Item) -> K,
{
    let mut iterator = items.into_iter();
    let mut kept = iterator.next()?;
    let mut kept_key = key(&kept);

    for item in iterator {
        let item_key = key(&item);
        if item_key > kept_key {
            kept = item;
            kept_key = item_key;
        }
    }

    Some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{cases, floats};

    #[test]
    fn max_first_uses_earliest_strictly_greater_key() {
        let all: Vec<(Vec<String>, Option<usize>)> = cases("order", "max_first");
        for (keys, expected_index) in all {
            let chosen = max_first(floats(&keys).into_iter().enumerate(), |(_, k)| *k);
            let index = chosen.map(|(index, _)| index);
            assert_eq!(index, expected_index, "keys: {keys:?}");
        }
    }
}
