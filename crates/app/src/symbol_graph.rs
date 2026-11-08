//! The resolved symbol graph that explain, path and health read: its build over one scan, the
//! graph of one call that a trace builds at most once, the order of its evidence tiers, and the
//! verbs of its relations.

use std::cell::OnceCell;

use graph::{FileFacts, NameIndex, Relation, Resolver, Scopes, SymbolGraph, Tier};
use kernel::{Deadline, Stopped};

use crate::Error;
use crate::scan::RepoFacts;

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

// The resolved graph of one call's scan, over the name index that the call built from it. The
// first card that asks builds it; every later card of the call reads the same graph.
pub(crate) struct CallGraph<'a> {
    pub(crate) facts: &'a RepoFacts,
    pub(crate) index: &'a NameIndex<'a>,
    scopes: &'a OnceCell<Scopes<'a>>,
    graph: &'a OnceCell<Result<SymbolGraph<'a>, Stopped>>,
    deadline: Option<&'a Deadline>,
}

impl<'a> CallGraph<'a> {
    // A stopped build stops every later card too, as the deadline it ran under has passed.
    pub(crate) fn graph(&self) -> Result<&SymbolGraph<'a>, Error> {
        let built = self.graph.get_or_init(|| {
            let scopes = self.scopes.get_or_init(|| Scopes::build(&self.facts.files));
            SymbolGraph::build(&Resolver::new(self.index, scopes), self.deadline)
        });
        built.as_ref().map_err(|stopped| Error::from(*stopped))
    }
}

// Gives `answer` the call graph of `facts` over `index`, built only when a card asks for it.
pub(crate) fn with_call_graph<T>(
    facts: &RepoFacts,
    index: &NameIndex<'_>,
    deadline: Option<&Deadline>,
    answer: impl FnOnce(&CallGraph<'_>) -> T,
) -> T {
    let (scopes, graph) = (OnceCell::new(), OnceCell::new());
    answer(&CallGraph {
        facts,
        index,
        scopes: &scopes,
        graph: &graph,
        deadline,
    })
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
