//! The owned form of the resolver's candidate order, which an update is compared by. The memo is
//! the history of the resolver's answers, not its state, and stays out.

use serde_json::{Value, json};

use super::Resolver;

impl Resolver<'_, '_> {
    // The candidates of each defined name, in name order, with their path groups.
    pub(crate) fn canonical(&self) -> Value {
        let mut rows: Vec<(&str, Value)> = (self.candidates.iter())
            .filter_map(|held| {
                let name = held.ordered.first()?.symbol().name.as_str();
                let ordered: Vec<Value> = (held.ordered.iter())
                    .map(|definition| json!([definition.path(), definition.position().1]))
                    .collect();
                Some((name, json!([name, ordered, held.groups])))
            })
            .collect();
        rows.sort_unstable_by(|left, right| left.0.cmp(right.0));
        Value::from(rows.into_iter().map(|(_, row)| row).collect::<Vec<_>>())
    }
}
