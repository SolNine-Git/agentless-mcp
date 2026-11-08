//! The owned form of a symbol graph, in list order, that an update is compared by.

use serde_json::{Value, json};

use super::SymbolGraph;
use crate::tests::corpus::{edge, endpoint, reference};

impl SymbolGraph<'_> {
    // Every list and map of the graph, each in its own order.
    pub(crate) fn canonical(&self) -> Vec<(&'static str, Value)> {
        let definitions: Vec<Value> = (self.definitions.iter())
            .map(|(id, definition)| json!([id, definition.path(), definition.position().1]))
            .collect();
        let candidates: Vec<Value> = (self.candidates.iter())
            .map(|(name, targets)| json!([name, targets.iter().map(endpoint).collect::<Vec<_>>()]))
            .collect();
        let candidates_at: Vec<Value> = (self.candidates_at.iter())
            .map(|(node, held)| {
                let held: Vec<Value> = (held.iter())
                    .map(|(name, target)| json!([name, endpoint(target)]))
                    .collect();
                json!([node.as_ref(), held])
            })
            .collect();
        vec![
            ("edges", self.edges.iter().map(edge).collect()),
            ("by_target", json!(self.by_target)),
            ("definitions", Value::from(definitions)),
            ("files", json!(self.files)),
            (
                "unresolved",
                json!([self.unresolved_imports, self.unresolved_internal_imports]),
            ),
            ("ambiguous", self.ambiguous.iter().map(reference).collect()),
            ("candidates", Value::from(candidates)),
            ("candidate_names", json!(self.candidate_names)),
            ("by_name", json!(self.by_name)),
            ("candidates_at", Value::from(candidates_at)),
        ]
    }
}
