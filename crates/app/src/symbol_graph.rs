//! The resolved symbol graph that explain, path and health read: its build over one scan, the
//! order of its evidence tiers, and the verbs of its relations.

use graph::{FileFacts, NameIndex, Relation, Resolver, Scopes, SymbolGraph, Tier};
use kernel::Deadline;

use crate::Error;

// Builds the name index and the resolved symbol graph of one scan, and gives both to `answer`.
pub(crate) fn with_resolved<T>(
    files: &[FileFacts],
    deadline: Option<&Deadline>,
    answer: impl FnOnce(&NameIndex<'_>, &SymbolGraph<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let index = NameIndex::build(files);
    let scopes = Scopes::build(files);
    let resolver = Resolver::new(&index, &scopes);
    let graph = SymbolGraph::build(&resolver, deadline)?;
    answer(&index, &graph)
}

// The position of `tier` in the strongest-first tier order.
pub(crate) fn tier_at(tier: Tier) -> usize {
    let at = Tier::ORDER.iter().position(|known| *known == tier);
    at.expect("every tier is in the tier order")
}

// The relation as a step with the edge reads it, and as a step against the edge reads it.
pub(crate) fn verbs(relation: Relation) -> (&'static str, &'static str) {
    match relation {
        Relation::References => ("references", "referenced by"),
        Relation::Imports => ("imports", "imported by"),
        Relation::Inherits => ("inherits", "inherited by"),
    }
}
