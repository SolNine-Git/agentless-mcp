//! The owned form of the aliases, in list order, that an update is compared by.

use serde_json::{Value, json};

use super::Aliases;

impl Aliases<'_> {
    // The bindings in their order, and each stable id with the bindings that reach it, in id order.
    pub(crate) fn canonical(&self) -> Value {
        let mut reaching: Vec<(&String, &Vec<usize>)> = self.reaching.iter().collect();
        reaching.sort_unstable();
        json!({ "bindings": self.bindings, "reaching": reaching })
    }
}
