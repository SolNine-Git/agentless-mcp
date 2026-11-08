//! The names that each file's `from x import member as local` binds, by the definitions that each
//! reaches through aliases and re-exports.

use std::collections::{BTreeSet, HashMap};

use crate::affected::Affected;
use crate::lists::merged;
use crate::names::Definition;
use crate::resolver::Resolver;
use crate::scan::FileFacts;
use crate::scopes::ImportScope;

/// Each `(path, local)` member binding whose import chain reaches a definition, by its stable id.
pub struct Aliases<'a> {
    bindings: Vec<(&'a str, &'a str)>,
    reaching: HashMap<String, Vec<usize>>,
}

impl<'a> Aliases<'a> {
    /// Follow every member binding of every file through [`Resolver::through_imports`].
    #[must_use]
    pub fn build(resolver: &Resolver<'_, 'a>) -> Aliases<'a> {
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

    // Follows again each member binding of the whole files of `affected`, and each binding whose
    // chain starts at a step that the update can change.
    pub(crate) fn update(
        &mut self,
        resolver: &Resolver<'_, 'a>,
        files: &[&'a FileFacts],
        affected: &Affected<'a>,
    ) {
        let scopes = resolver.scopes();
        let whole: BTreeSet<&str> = affected
            .whole
            .iter()
            .map(|&at| files[at].path.as_str())
            .collect();
        let mut again: BTreeSet<(&'a str, &'a str)> = BTreeSet::new();
        for &at in &affected.whole {
            let path = files[at].path.as_str();
            let bound = scopes
                .get(path)
                .into_iter()
                .flat_map(ImportScope::member_names);
            again.extend(bound.map(|local| (path, local)));
        }
        let binds = |(path, local): &&(&str, &str)| {
            scopes
                .get(path)
                .is_some_and(|scope| scope.members.contains_key(local))
        };
        again.extend(affected.steps.iter().filter(binds));
        let stale = |binding: &(&str, &str)| again.contains(binding) || whole.contains(binding.0);
        let removed: Vec<usize> = (self.bindings.iter().enumerate())
            .filter(|(_, binding)| stale(binding))
            .map(|(at, _)| at)
            .collect();
        let found: Vec<((&'a str, &'a str), Vec<Definition<'a>>)> = (again.into_iter())
            .map(|(path, local)| ((path, local), resolver.through_imports(local, path)))
            .filter(|(_, reached)| !reached.is_empty())
            .collect();
        let added = found.iter().map(|(binding, _)| *binding).collect();
        let (to, placed) = merged(&mut self.bindings, &removed, added, Ord::cmp);
        for held in self.reaching.values_mut() {
            held.retain_mut(|at| {
                *at = to[*at];
                *at != usize::MAX
            });
        }
        for (at, (_, reached)) in placed.into_iter().zip(&found) {
            for definition in reached {
                let held = self.reaching.entry(definition.stable_id().to_string());
                held.or_default().push(at);
            }
        }
        for held in self.reaching.values_mut() {
            held.sort_unstable();
            held.dedup();
        }
        self.reaching.retain(|_, held| !held.is_empty());
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
