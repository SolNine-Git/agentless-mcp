//! What one update resolves again: the files whose every site it resolves, and the names whose
//! every site it resolves, from what the changed files change in their definitions and scopes.

use std::collections::{BTreeMap, BTreeSet};

use facts::{StableId, Symbol, SymbolKind};

use crate::names::NameIndex;
use crate::scan::{FileFacts, in_name_graph};
use crate::scopes::{ImportScope, Scopes};

// The work of one update.
pub(crate) struct Affected<'s> {
    // The changed files, each file whose scope changed, and each file that reads every name of a
    // file whose scope changed.
    pub(crate) whole: BTreeSet<usize>,
    // The names whose sites resolve again in every file.
    pub(crate) names: BTreeSet<&'s str>,
    // Each symbol of a changed file that kept its id at another line, with the new line.
    pub(crate) moved: BTreeMap<String, u32>,
    // Each member step whose answer the update can change, by the file asked and the name.
    pub(crate) steps: BTreeSet<(&'s str, &'s str)>,
    pub(crate) changed: Vec<usize>,
}

// A chain step that an update changed: a member name asked of a file, or every name of it.
type Hop<'s> = (&'s str, Option<&'s str>);

impl<'s> Affected<'s> {
    pub(crate) fn of(
        before: &[&'s FileFacts],
        after: &[&'s FileFacts],
        changed: &[usize],
        scopes: Option<(&Scopes<'s>, &[(usize, ImportScope<'s>)])>,
    ) -> Affected<'s> {
        let mut affected = Affected {
            whole: changed.iter().copied().collect(),
            names: BTreeSet::new(),
            moved: BTreeMap::new(),
            steps: BTreeSet::new(),
            changed: changed.to_vec(),
        };
        let mut hops: Vec<Hop<'s>> = Vec::new();
        for &at in changed {
            let (old, new) = (before[at], after[at]);
            let defined = defined_names(old, new);
            hops.extend(defined.iter().map(|name| (new.path.as_str(), Some(*name))));
            affected.names.extend(defined);
            affected.moved.extend(moved(old, new));
        }
        let Some((scopes, rebound)) = scopes else {
            return affected;
        };
        for (at, old) in rebound {
            affected.whole.insert(*at);
            hops.extend(changed_hops(after[*at].path.as_str(), old, scopes.at(*at)));
        }
        affected.reach(scopes, after, hops);
        affected
    }

    // Whether the update would resolve more than `percent` of the scan's sites again: every site of
    // the whole files, and each other site of the affected names.
    pub(crate) fn costs_more_than(
        &self,
        percent: usize,
        index: &NameIndex<'s>,
        files: &[&FileFacts],
    ) -> bool {
        let whole: usize = self
            .whole
            .iter()
            .map(|&at| files[at].facts.refs.len())
            .sum();
        let named = self.names.iter().flat_map(|name| index.places(name));
        let outside = named.filter(|(at, _)| !self.whole.contains(at)).count();
        let total: usize = files.iter().map(|file| file.facts.refs.len()).sum();
        (whole + outside) * 100 > percent * total
    }

    // Follows each changed step back to every member or whole-module binding that can reach it. A
    // file that reads every name of a file whose names all changed resolves again as a whole.
    fn reach(&mut self, scopes: &Scopes<'s>, files: &[&'s FileFacts], hops: Vec<Hop<'s>>) {
        let mut seen: BTreeSet<Hop<'s>> = BTreeSet::new();
        let mut pending = hops;
        while let Some(hop) = pending.pop() {
            if !seen.insert(hop) {
                continue;
            }
            let (target, member) = hop;
            self.names.extend(member);
            self.steps.extend(member.map(|member| (target, member)));
            let importers: Vec<usize> = scopes.importers(target).collect();
            for &importer in &importers {
                let path = files[importer].path.as_str();
                pending.extend(steps_back(scopes.at(importer), path, hop));
            }
            if member.is_none() {
                self.whole.extend(importers);
            }
        }
    }
}

// The steps of the file at `path` whose answer reads the answer of `hop`: each local name that a
// member binding of the step binds, and the same step through a whole-module binding.
fn steps_back<'s>(scope: &ImportScope<'s>, path: &'s str, hop: Hop<'s>) -> Vec<Hop<'s>> {
    let (target, member) = hop;
    let reads = |(held, name): &(&str, &str)| {
        *held == target && member.is_none_or(|member| member == *name)
    };
    let bound = scope
        .members
        .iter()
        .filter(|(_, pairs)| pairs.iter().any(reads));
    let mut steps: Vec<Hop<'s>> = bound.map(|(local, _)| (path, Some(*local))).collect();
    if scope.wholesale.contains(target) {
        steps.push((path, member));
    }
    steps
}

// The steps that a changed scope answers differently: each member name whose explicit binding
// changed, and every name when the whole-module bindings changed.
fn changed_hops<'s>(path: &'s str, old: &ImportScope<'s>, new: &ImportScope<'s>) -> Vec<Hop<'s>> {
    if old.wholesale != new.wholesale {
        return vec![(path, None)];
    }
    let names: BTreeSet<&'s str> = old
        .members
        .keys()
        .chain(new.members.keys())
        .copied()
        .collect();
    let differing = names
        .into_iter()
        .filter(|name| old.members.get(name) != new.members.get(name));
    differing.map(|name| (path, Some(name))).collect()
}

// What a resolution reads of one definition: its identity, and whether it shares its line with
// the definition before it in candidate order.
type Shape<'s> = (SymbolKind, &'s str, String, bool);

// The names whose definitions differ in what a resolution reads: their identities in candidate
// order, and which of them share a line, as a chain keeps one definition of each line it reaches.
fn defined_names<'s>(old: &'s FileFacts, new: &'s FileFacts) -> BTreeSet<&'s str> {
    if !in_name_graph(new) {
        return BTreeSet::new();
    }
    let (before, after) = (shapes(&old.facts.symbols), shapes(&new.facts.symbols));
    let names = before.keys().chain(after.keys()).copied();
    let mut differing: BTreeSet<&'s str> = names
        .filter(|name| before.get(name) != after.get(name))
        .collect();
    let (was, is) = (
        shared_lines(&old.facts.symbols),
        shared_lines(&new.facts.symbols),
    );
    for group in was.symmetric_difference(&is) {
        differing.extend(group.iter().map(|(name, _)| *name));
    }
    differing
}

fn shapes(symbols: &[Symbol]) -> BTreeMap<&str, Vec<Shape<'_>>> {
    let mut by_name: BTreeMap<&str, Vec<(u32, String, usize)>> = BTreeMap::new();
    for (at, symbol) in symbols.iter().enumerate() {
        let held = by_name.entry(symbol.name.as_str()).or_default();
        held.push((symbol.line, symbol.qualname(), at));
    }
    let shaped = by_name.into_iter().map(|(name, mut held)| {
        held.sort();
        let lines: Vec<u32> = held.iter().map(|(line, _, _)| *line).collect();
        let shapes = held.iter().enumerate().map(|(place, (line, _, at))| {
            let symbol = &symbols[*at];
            let shares = place > 0 && lines[place - 1] == *line;
            (
                symbol.kind,
                symbol.parent.as_str(),
                symbol.id_qualname(),
                shares,
            )
        });
        (name, shapes.collect())
    });
    shaped.collect()
}

// Each group of two or more top-level definitions on one line, by name and id.
fn shared_lines(symbols: &[Symbol]) -> BTreeSet<Vec<(&str, String)>> {
    let mut by_line: BTreeMap<u32, Vec<(&str, String)>> = BTreeMap::new();
    for symbol in symbols.iter().filter(|symbol| symbol.parent.is_empty()) {
        let held = by_line.entry(symbol.line).or_default();
        held.push((symbol.name.as_str(), symbol.id_qualname()));
    }
    by_line
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

// The symbols of the file that keep their id at another line.
fn moved(old: &FileFacts, new: &FileFacts) -> Vec<(String, u32)> {
    if !in_name_graph(new) {
        return Vec::new();
    }
    let id = |symbol: &Symbol| StableId::of(new.language, &new.path, symbol).to_string();
    let lines: BTreeMap<String, u32> = (old.facts.symbols.iter())
        .map(|symbol| (id(symbol), symbol.line))
        .collect();
    let kept = new
        .facts
        .symbols
        .iter()
        .map(|symbol| (id(symbol), symbol.line));
    let moved = kept.filter(|(id, line)| lines.get(id).is_some_and(|held| held != line));
    moved.collect()
}
