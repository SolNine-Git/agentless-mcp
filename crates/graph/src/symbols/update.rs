//! The update of a kept symbol graph: the links of each occurrence that an update resolves again
//! take the place of its old links, and every list keeps the order that a full build gives it.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use facts::StableId;

use super::{AmbiguousReference, Endpoint, Met, Nodes, Parts, Relation, SymbolEdge, SymbolGraph};
use crate::affected::Affected;
use crate::lists::{interleaved, merged};
use crate::names::NameIndex;
use crate::resolver::Resolver;
use crate::scan::{FileFacts, in_name_graph};
use crate::scopes::{ImportScope, Scopes};
use crate::unresolved::{repository_segments, unresolved_rows};

// The occurrences of one file that an update links again: all of them, or those of some names.
type Wanted<'a> = Option<BTreeSet<&'a str>>;

// The candidates that an update gives a name: a new first meeting, none, or the kept ones.
enum Chosen<'a> {
    New(Met, Vec<Endpoint<'a>>),
    Gone,
    Kept,
}

// What an update links again, before it changes the graph.
pub(crate) struct Relinked<'n, 'a> {
    parts: Parts<'n, 'a>,
    removed_edges: Vec<usize>,
    removed_ambiguous: Vec<usize>,
    chosen: BTreeMap<&'a str, Chosen<'a>>,
    // The names whose ambiguous references or candidate nodes differ after the update.
    tallied: Vec<&'a str>,
}

impl<'a> Relinked<'_, 'a> {
    // How many references and bases the update resolved again.
    pub(crate) fn sites(&self) -> usize {
        self.parts.sites
    }

    // The names whose ambiguous edges the update changes.
    pub(crate) fn names(&self) -> &[&'a str] {
        &self.tallied
    }
}

impl<'a> SymbolGraph<'a> {
    // Links the occurrences that `affected` names again over the new scan of `nodes`, and finds
    // what they replace. `before` is the scan of the graph.
    pub(crate) fn relinked<'n>(
        &self,
        nodes: &'n Nodes<'a>,
        resolver: &Resolver<'_, 'a>,
        before: &[&'a FileFacts],
        affected: &Affected<'a>,
    ) -> Relinked<'n, 'a> {
        let touched = self.touched(resolver.index(), affected);
        let mut parts = Parts::new(nodes);
        for (&at, wanted) in &touched {
            linked(&mut parts, resolver, at, wanted.as_ref(), affected);
        }
        let (removed_edges, removed_ambiguous) = self.removed(before, &touched);
        parts.ambiguous.sort_by(AmbiguousReference::cmp_total);
        parts.ambiguous.dedup();
        let mut changing: BTreeSet<&'a str> = affected.names.clone();
        changing.extend(parts.ambiguous.iter().map(AmbiguousReference::name));
        changing.extend(removed_ambiguous.iter().map(|&at| self.ambiguous[at].name));
        let chosen: BTreeMap<&'a str, Chosen<'a>> = (changing.into_iter())
            .map(|name| (name, self.chosen(name, &parts, resolver, affected)))
            .collect();
        let tallied = self.tallied(&chosen, &parts.ambiguous, &removed_ambiguous);
        Relinked {
            parts,
            removed_edges,
            removed_ambiguous,
            chosen,
            tallied,
        }
    }

    // The chosen names whose degrees can change: the source nodes of their ambiguous references,
    // sorted `added` against the kept ones at `removed`, or the nodes of their candidates.
    fn tallied(
        &self,
        chosen: &BTreeMap<&'a str, Chosen<'a>>,
        added: &[AmbiguousReference<'a>],
        removed: &[usize],
    ) -> Vec<&'a str> {
        let mut referenced: BTreeMap<&str, (Vec<&str>, Vec<&str>)> = BTreeMap::new();
        for &at in removed {
            let reference = &self.ambiguous[at];
            let held = referenced.entry(reference.name).or_default();
            held.0.push(reference.source.node());
        }
        for reference in added {
            let held = referenced.entry(reference.name).or_default();
            held.1.push(reference.source.node());
        }
        let nodes = |targets: &[Endpoint<'_>]| -> Vec<String> {
            targets
                .iter()
                .map(|target| target.node().to_owned())
                .collect()
        };
        let old = |name: &str| {
            self.candidate_names
                .get(name)
                .map(|&at| nodes(&self.candidates[at].1))
        };
        let differs = |name: &&'a str, choice: &Chosen<'a>| {
            let refs = referenced.get(*name).is_some_and(|(was, is)| was != is);
            let targets = match choice {
                Chosen::Kept => false,
                Chosen::Gone => old(name).is_some(),
                Chosen::New(_, targets) => old(name) != Some(nodes(targets)),
            };
            refs || targets
        };
        let changed = chosen.iter().filter(|(name, choice)| differs(name, choice));
        changed.map(|(name, _)| *name).collect()
    }

    // Puts the links of `relinked` in place of the links they replace, and moves the endpoints of
    // the moved symbols of `affected`.
    pub(crate) fn relink(
        &mut self,
        relinked: Relinked<'_, 'a>,
        before: &[&'a FileFacts],
        affected: &Affected<'a>,
    ) {
        let Relinked {
            mut parts,
            removed_edges,
            removed_ambiguous,
            chosen,
            tallied: _,
        } = relinked;
        parts.edges.sort_by(SymbolEdge::cmp_total);
        parts.edges.dedup();
        let edges = std::mem::take(&mut parts.edges);
        let (to, added) = merged(
            &mut self.edges,
            &removed_edges,
            edges,
            SymbolEdge::cmp_total,
        );
        self.retarget(&to, added);
        let ambiguous = std::mem::take(&mut parts.ambiguous);
        let order = AmbiguousReference::cmp_total;
        let (to, added) = merged(&mut self.ambiguous, &removed_ambiguous, ambiguous, order);
        self.renamed(&to, &added);
        for &at in &affected.changed {
            self.undefined(before[at], at);
        }
        self.definitions.extend(parts.definitions);
        for (name, files) in parts.bases {
            let held = self.bases.entry(name).or_default();
            held.extend(files);
            held.sort_unstable();
            held.dedup();
        }
        self.rechosen(chosen);
        self.refreshed(&affected.moved);
    }

    // Puts the import edges of each file whose statement rows changed in place of its old ones,
    // in an import graph. Returns whether a pair of importing and imported files changed.
    pub(crate) fn reimported(
        &mut self,
        scopes: &Scopes<'a>,
        rebound: &[(usize, ImportScope<'a>)],
    ) -> bool {
        let restated: Vec<usize> = (rebound.iter())
            .filter(|(at, old)| old.statements != scopes.at(*at).statements)
            .map(|(at, _)| *at)
            .collect();
        let files = scopes.files();
        let nodes = Nodes::new(files, false);
        let mut parts = Parts::new(&nodes);
        let mut removed = Vec::new();
        for &at in &restated {
            parts.import_edges(scopes, at);
            removed.extend(run(&self.edges, |edge| edge.source.node(), &files[at].path));
        }
        removed.sort_unstable();
        let pairs =
            |edges: &mut dyn Iterator<Item = &SymbolEdge<'a>>| -> BTreeSet<(&'a str, &'a str)> {
                edges
                    .map(|edge| (edge.source.path, edge.target.path))
                    .collect()
            };
        let before = pairs(&mut removed.iter().map(|&at| &self.edges[at]));
        parts.edges.sort_by(SymbolEdge::cmp_total);
        parts.edges.dedup();
        let after = pairs(&mut parts.edges.iter());
        let (to, added) = merged(
            &mut self.edges,
            &removed,
            parts.edges,
            SymbolEdge::cmp_total,
        );
        self.retarget(&to, added);
        self.recount(scopes, rebound);
        before != after
    }

    // The unresolved counts after the scopes of `rebound` changed, each with its scope before.
    pub(crate) fn recount(&mut self, scopes: &Scopes<'a>, rebound: &[(usize, ImportScope<'a>)]) {
        let unresolved =
            |scope: &ImportScope<'_>| scope.statements.iter().any(|row| row.resolved.is_none());
        let reads = rebound
            .iter()
            .any(|(at, old)| unresolved(old) || unresolved(scopes.at(*at)));
        if !reads {
            return;
        }
        let segments = repository_segments(scopes.files().iter().map(|file| file.path.as_str()));
        for (at, old) in rebound {
            let (was, is) = (
                unresolved_rows(old, &segments),
                unresolved_rows(scopes.at(*at), &segments),
            );
            self.unresolved_imports = self.unresolved_imports + is.0 - was.0;
            self.unresolved_internal_imports = self.unresolved_internal_imports + is.1 - was.1;
        }
    }

    // Each file with the occurrences an update links again: every occurrence of a whole file, and
    // in every other file the occurrences and bases of the affected names.
    fn touched(
        &self,
        index: &NameIndex<'a>,
        affected: &Affected<'a>,
    ) -> BTreeMap<usize, Wanted<'a>> {
        let mut touched: BTreeMap<usize, Wanted<'a>> =
            affected.whole.iter().map(|&at| (at, None)).collect();
        for &name in &affected.names {
            let spelled = index.sites_at(name).map(|(at, _)| at);
            let based = self.bases.get(name).into_iter().flatten().copied();
            for at in spelled.chain(based) {
                wants(&mut touched, at, name);
            }
        }
        touched
    }

    // The positions of the kept edges and ambiguous references that the touched occurrences
    // made, each list ascending. Every link of a file starts at the file or at one of its symbols.
    fn removed(
        &self,
        before: &[&'a FileFacts],
        touched: &BTreeMap<usize, Wanted<'a>>,
    ) -> (Vec<usize>, Vec<usize>) {
        let (mut edges, mut ambiguous) = (Vec::new(), Vec::new());
        for (&at, wanted) in touched {
            for node in source_nodes(before[at]) {
                self.made(&node, wanted.as_ref(), &mut edges, &mut ambiguous);
            }
        }
        for list in [&mut edges, &mut ambiguous] {
            list.sort_unstable();
            list.dedup();
        }
        (edges, ambiguous)
    }

    // The positions of the edges and ambiguous references from `node` that `wanted` names.
    fn made(
        &self,
        node: &str,
        wanted: Option<&BTreeSet<&str>>,
        edges: &mut Vec<usize>,
        ambiguous: &mut Vec<usize>,
    ) {
        let named = |name: &str| wanted.is_none_or(|names| names.contains(name));
        let linked = |&at: &usize| {
            let edge = &self.edges[at];
            wanted.is_none() || (edge.relation != Relation::Imports && named(edge.name()))
        };
        edges.extend(run(&self.edges, |edge| edge.source.node(), node).filter(linked));
        let held = run(&self.ambiguous, |item| item.source.node(), node);
        ambiguous.extend(held.filter(|&at| named(self.ambiguous[at].name)));
    }

    // The candidates of `name` after the update: the first ambiguous occurrence of the name in
    // scan order decides them, as in a full build.
    fn chosen(
        &self,
        name: &'a str,
        parts: &Parts<'_, 'a>,
        resolver: &Resolver<'_, 'a>,
        affected: &Affected<'a>,
    ) -> Chosen<'a> {
        let new = (parts.candidate_names.get(name))
            .map(|&at| (parts.met[at], parts.candidates[at].1.clone()));
        let old = self.candidate_names.get(name).map(|&at| self.met[at]);
        let first = match old {
            _ if affected.names.contains(name) => new,
            None => new,
            Some(old) if !affected.whole.contains(&old.0) => match new {
                Some((met, targets)) if met < old => Some((met, targets)),
                _ => return Chosen::Kept,
            },
            Some(_) => {
                let outside = self.first_outside(name, parts.nodes, resolver, affected);
                [new, outside]
                    .into_iter()
                    .flatten()
                    .min_by_key(|(met, _)| *met)
            }
        };
        first.map_or(Chosen::Gone, |(met, targets)| Chosen::New(met, targets))
    }

    // The first ambiguous occurrence of `name` in the files that the update does not link again
    // as a whole, which keep their references of the name.
    fn first_outside(
        &self,
        name: &'a str,
        nodes: &Nodes<'a>,
        resolver: &Resolver<'_, 'a>,
        affected: &Affected<'a>,
    ) -> Option<(Met, Vec<Endpoint<'a>>)> {
        let scopes = resolver.scopes();
        let held = self.by_name.get(name).into_iter().flatten();
        let files = held.filter_map(|&at| scopes.position(self.ambiguous[at].source.path()));
        let first = files.filter(|at| !affected.whole.contains(at)).min()?;
        let mut parts = Parts::new(nodes);
        let names = BTreeSet::from([name]);
        let linked = parts.symbol_links(resolver, first, Some(&names), None);
        linked.expect("an update has no deadline");
        let at = *parts.candidate_names.get(name)?;
        Some((parts.met[at], parts.candidates.swap_remove(at).1))
    }

    // The positions of `by_target` after a merge: each kept edge at its new position, and each
    // added edge where its target and position put it.
    fn retarget(&mut self, to: &[usize], mut added: Vec<usize>) {
        (self.by_target).retain_mut(|at| {
            *at = to[*at];
            *at != usize::MAX
        });
        let edges = &self.edges;
        let key = |at: usize| (edges[at].target.node(), at);
        added.sort_by(|left, right| key(*left).cmp(&key(*right)));
        let points: Vec<usize> = (added.iter())
            .map(|&at| self.by_target.partition_point(|&held| key(held) < key(at)))
            .collect();
        self.by_target = interleaved(std::mem::take(&mut self.by_target), added, &points);
    }

    // The positions of `by_name` after a merge of the ambiguous references.
    fn renamed(&mut self, to: &[usize], added: &[usize]) {
        for held in self.by_name.values_mut() {
            held.retain_mut(|at| {
                *at = to[*at];
                *at != usize::MAX
            });
        }
        for &at in added {
            self.by_name
                .entry(self.ambiguous[at].name)
                .or_default()
                .push(at);
        }
        for held in self.by_name.values_mut() {
            held.sort_unstable();
        }
        self.by_name.retain(|_, held| !held.is_empty());
    }

    // Takes out the definitions and the bases of the file at `at` before the update.
    fn undefined(&mut self, file: &'a FileFacts, at: usize) {
        if !in_name_graph(file) {
            return;
        }
        for symbol in &file.facts.symbols {
            let id = StableId::of(file.language, &file.path, symbol).to_string();
            self.definitions.remove(&id);
            let base = symbol.bases.iter().map(|base| facts::base_name(base));
            for base in base.filter(|base| !base.is_empty()) {
                self.unbased(base, at);
            }
        }
    }

    fn unbased(&mut self, base: &str, at: usize) {
        let Some(held) = self.bases.get_mut(base) else {
            return;
        };
        held.retain(|file| *file != at);
        if held.is_empty() {
            self.bases.remove(base);
        }
    }

    // Puts each chosen name's candidates in its place in first-met order.
    fn rechosen(&mut self, chosen: BTreeMap<&'a str, Chosen<'a>>) {
        let mut out: Vec<usize> = Vec::new();
        let mut rows: Vec<(Met, (&'a str, Vec<Endpoint<'a>>))> = Vec::new();
        for (name, choice) in chosen {
            if matches!(choice, Chosen::Kept) {
                continue;
            }
            if let Some(&at) = self.candidate_names.get(name) {
                out.push(at);
                self.unplaced(at);
            }
            if let Chosen::New(met, targets) = choice {
                rows.push((met, (name, targets)));
            }
        }
        out.sort_unstable();
        rows.sort_by_key(|(met, _)| *met);
        let held: Vec<(Met, (&'a str, Vec<Endpoint<'a>>))> = (self.met.drain(..))
            .zip(self.candidates.drain(..))
            .collect();
        let mut held = held;
        let (to, added) = merged(&mut held, &out, rows, |left, right| left.0.cmp(&right.0));
        (self.met, self.candidates) = held.into_iter().unzip();
        let names = &mut self.candidate_names;
        names.retain(|_, at| to[*at] != usize::MAX);
        for at in names.values_mut() {
            *at = to[*at];
        }
        for &at in &added {
            names.insert(self.candidates[at].0, at);
        }
        for at in added {
            self.placed(at);
        }
    }

    // Takes the candidate endpoints of the row at `at` out of `candidates_at`.
    fn unplaced(&mut self, at: usize) {
        let (name, targets) = &self.candidates[at];
        for target in targets {
            let Some(held) = self.candidates_at.get_mut(target.node()) else {
                continue;
            };
            held.retain(|(held, _)| held != name);
            if held.is_empty() {
                self.candidates_at.remove(target.node());
            }
        }
    }

    // Puts the candidate endpoints of the row at `at` in `candidates_at`, each list in the order
    // that its names were first met.
    fn placed(&mut self, at: usize) {
        let (name, targets) = &self.candidates[at];
        let met = self.met[at];
        for target in targets {
            let node = std::sync::Arc::clone(target.node_arc());
            let held = self.candidates_at.entry(node).or_default();
            let names = &self.candidate_names;
            let first = |name: &str| self.met[names[name]];
            let point = held.partition_point(|(held, _)| first(held) < met);
            held.insert(point, (*name, target.clone()));
        }
    }

    // Each kept endpoint of a moved symbol takes the symbol's new line.
    fn refreshed(&mut self, moved: &BTreeMap<String, u32>) {
        for (id, &line) in moved {
            let edges = &self.edges;
            let start =
                (self.by_target).partition_point(|&at| edges[at].target.node() < id.as_str());
            let end =
                (self.by_target).partition_point(|&at| edges[at].target.node() <= id.as_str());
            for &at in &self.by_target[start..end] {
                self.edges[at].target.line = line;
            }
            let Some(held) = self.candidates_at.get_mut(id.as_str()) else {
                continue;
            };
            for (name, target) in held.iter_mut() {
                target.line = line;
                let row = &mut self.candidates[self.candidate_names[name]].1;
                let at = row.iter_mut().filter(|endpoint| endpoint.node() == id);
                at.for_each(|endpoint| endpoint.line = line);
            }
        }
    }
}

// Marks the occurrences of `name` in the file at `at` for linking again, unless the whole file is.
fn wants<'a>(touched: &mut BTreeMap<usize, Wanted<'a>>, at: usize, name: &'a str) {
    match touched.entry(at) {
        Entry::Vacant(free) => {
            free.insert(Some(BTreeSet::from([name])));
        }
        Entry::Occupied(mut held) => {
            if let Some(names) = held.get_mut() {
                names.insert(name);
            }
        }
    }
}

// Links the occurrences of the file at `at` that `wanted` names, as a build links the file.
fn linked<'a>(
    parts: &mut Parts<'_, 'a>,
    resolver: &Resolver<'_, 'a>,
    at: usize,
    wanted: Option<&BTreeSet<&str>>,
    affected: &Affected<'a>,
) {
    if wanted.is_none() {
        parts.import_edges(resolver.scopes(), at);
    }
    if !in_name_graph(parts.nodes.files[at]) {
        return;
    }
    if affected.changed.contains(&at) {
        parts.definitions_of(at);
    }
    let linked = parts.symbol_links(resolver, at, wanted, None);
    linked.expect("an update has no deadline");
}

// The nodes that a file's links start at: the file, and each of its symbols.
fn source_nodes(file: &FileFacts) -> Vec<String> {
    let mut nodes = vec![file.path.clone()];
    if in_name_graph(file) {
        let ids = file.facts.symbols.iter();
        nodes.extend(ids.map(|symbol| StableId::of(file.language, &file.path, symbol).to_string()));
    }
    nodes
}

// The positions of the items of `held`, sorted by `key` first, whose key is `node`.
fn run<T>(held: &[T], key: impl Fn(&T) -> &str, node: &str) -> std::ops::Range<usize> {
    let start = held.partition_point(|item| key(item) < node);
    let end = held.partition_point(|item| key(item) <= node);
    start..end
}
