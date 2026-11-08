//! The names that each file's `from x import member as local` binds, by the definitions that each
//! reaches through aliases and re-exports.

use std::collections::{BTreeSet, HashMap};

use crate::names::Definition;
use crate::resolver::Resolver;

/// Each `(path, local)` member binding whose import chain reaches a definition, by its stable id.
pub struct Aliases<'a> {
    bindings: Vec<(&'a str, &'a str)>,
    reaching: HashMap<String, Vec<usize>>,
}

impl<'a> Aliases<'a> {
    /// Follow every member binding of every file through [`Resolver::through_imports`].
    #[must_use]
    pub fn build(resolver: &Resolver<'a>) -> Aliases<'a> {
        let bindings = (resolver.scopes().iter())
            .flat_map(|(path, scope)| scope.member_names().map(move |local| (path, local)));
        let mut aliases = Aliases {
            bindings: Vec::new(),
            reaching: HashMap::new(),
        };
        for (path, local) in bindings {
            aliases.add((path, local), &resolver.through_imports(local, path));
        }
        for held in aliases.reaching.values_mut() {
            held.dedup();
        }
        aliases
    }

    /// Return each binding that reaches one of `stable_ids` once, in the order of `Scopes::iter`.
    #[must_use]
    pub fn reaching(&self, stable_ids: &BTreeSet<String>) -> Vec<(&'a str, &'a str)> {
        let mut at: Vec<usize> = (stable_ids.iter())
            .filter_map(|id| self.reaching.get(id))
            .flatten()
            .copied()
            .collect();
        at.sort_unstable();
        at.dedup();
        at.into_iter().map(|at| self.bindings[at]).collect()
    }

    fn add(&mut self, binding: (&'a str, &'a str), reached: &[Definition<'a>]) {
        if reached.is_empty() {
            return;
        }
        let at = self.bindings.len();
        self.bindings.push(binding);
        for definition in reached {
            let held = self.reaching.entry(definition.stable_id().to_string());
            held.or_default().push(at);
        }
    }
}

#[cfg(test)]
mod canonical;
