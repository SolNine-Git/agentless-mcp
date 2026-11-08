//! The symbol graph: every resolved reference, import and base class of one scan, as edges
//! between symbols and files, each with the evidence tier behind it.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use facts::Ref;
use kernel::{Deadline, Stopped, checkpoint};

use crate::names::Definition;
use crate::owners::LineOwners;
use crate::resolver::{Resolution, Resolver, Tier};
use crate::scan::{FileFacts, in_name_graph};
use crate::scopes::Scopes;
use crate::unresolved::{unresolved_imports, unresolved_internal_imports};

const RELATION_VALUES: [&str; 3] = ["imports", "inherits", "references"];

/// What one edge says its source does to its target; the order is the order of the wire values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    Imports,
    Inherits,
    References,
}

impl Relation {
    /// Return the wire value, such as `references`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        RELATION_VALUES[self as usize]
    }
}

/// One end of an edge: a symbol, or a whole file at one line (0 for no line).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint<'a> {
    node: Arc<str>,
    path: &'a str,
    line: u32,
    label: Arc<str>,
    is_symbol: bool,
}

impl<'a> Endpoint<'a> {
    /// Return the node identity: a stable id for a symbol, the path for a file.
    #[must_use]
    pub fn node(&self) -> &str {
        &self.node
    }

    /// Return the repository-relative path.
    #[must_use]
    pub fn path(&self) -> &'a str {
        self.path
    }

    /// Return the line: the symbol's first line, or the file line the edge comes from.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    /// Return the label a reader sees: the qualified name, or the path.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Return whether the endpoint is a symbol rather than a whole file.
    #[must_use]
    pub fn is_symbol(&self) -> bool {
        self.is_symbol
    }

    pub(crate) fn node_arc(&self) -> &Arc<str> {
        &self.node
    }

    // The fields the sort key leaves out, so that a sort over endpoints is total.
    fn cmp_rest(&self, other: &Self) -> Ordering {
        (self.path, &self.label, self.is_symbol).cmp(&(other.path, &other.label, other.is_symbol))
    }
}

/// One resolved relationship, with the evidence tier behind it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SymbolEdge<'a> {
    source: Endpoint<'a>,
    target: Endpoint<'a>,
    name: Cow<'a, str>,
    relation: Relation,
    tier: Tier,
}

impl<'a> SymbolEdge<'a> {
    // An edge from a node to itself says nothing about how two things relate.
    pub(crate) fn new(
        source: Endpoint<'a>,
        target: Endpoint<'a>,
        name: Cow<'a, str>,
        relation: Relation,
        tier: Tier,
    ) -> Option<SymbolEdge<'a>> {
        (source.node != target.node).then_some(SymbolEdge {
            source,
            target,
            name,
            relation,
            tier,
        })
    }

    /// Return the endpoint the relationship starts at.
    #[must_use]
    pub fn source(&self) -> &Endpoint<'a> {
        &self.source
    }

    /// Return the endpoint the relationship points at.
    #[must_use]
    pub fn target(&self) -> &Endpoint<'a> {
        &self.target
    }

    /// Return the spelled name, or the module string of an import.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return what the source does to the target.
    #[must_use]
    pub fn relation(&self) -> Relation {
        self.relation
    }

    /// Return the evidence tier.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Compare by the sort key: source node, target node, relation, tier, name, source line,
    /// target line.
    #[must_use]
    pub fn cmp_sort_key(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }

    fn sort_key(&self) -> (&str, &str, Relation, Tier, &str, u32, u32) {
        let (source, target) = (&self.source, &self.target);
        let nodes = (source.node(), target.node());
        (
            nodes.0,
            nodes.1,
            self.relation,
            self.tier,
            self.name(),
            source.line,
            target.line,
        )
    }

    fn cmp_total(&self, other: &Self) -> Ordering {
        self.cmp_sort_key(other)
            .then_with(|| self.source.cmp_rest(&other.source))
            .then_with(|| self.target.cmp_rest(&other.target))
    }
}

/// One name-only-ambiguous reference: it stands for an edge to every definition of `name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AmbiguousReference<'a> {
    source: Endpoint<'a>,
    name: &'a str,
    relation: Relation,
}

impl<'a> AmbiguousReference<'a> {
    /// Return the endpoint that spells the name.
    #[must_use]
    pub fn source(&self) -> &Endpoint<'a> {
        &self.source
    }

    /// Return the spelled name.
    #[must_use]
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// Return what the source does to each candidate.
    #[must_use]
    pub fn relation(&self) -> Relation {
        self.relation
    }

    fn cmp_total(&self, other: &Self) -> Ordering {
        self.sort_key()
            .cmp(&other.sort_key())
            .then_with(|| self.source.cmp_rest(&other.source))
    }

    fn sort_key(&self) -> (&str, Relation, &str, u32) {
        (
            self.source.node(),
            self.relation,
            self.name,
            self.source.line,
        )
    }
}

/// Every resolved edge of one scan in one deterministic order, the ambiguous references kept
/// once each and expanded on demand.
pub struct SymbolGraph<'a> {
    pub(crate) edges: Vec<SymbolEdge<'a>>,
    by_target: Vec<usize>,
    definitions: BTreeMap<String, Definition<'a>>,
    files: Vec<&'a str>,
    unresolved_imports: u32,
    unresolved_internal_imports: u32,
    pub(crate) ambiguous: Vec<AmbiguousReference<'a>>,
    pub(crate) candidates: Vec<(&'a str, Vec<Endpoint<'a>>)>,
    // Where each name's candidates were met first, beside `candidates`: the order of the list.
    pub(crate) met: Vec<Met>,
    pub(crate) candidate_names: BTreeMap<&'a str, usize>,
    pub(crate) by_name: BTreeMap<&'a str, Vec<usize>>,
    pub(crate) candidates_at: BTreeMap<Arc<str>, Vec<(&'a str, Endpoint<'a>)>>,
    // The scan positions of the files whose symbols name each base, in scan order.
    bases: BTreeMap<&'a str, Vec<usize>>,
}

// Where one link is made: the scan position of its file, and its place among the file's links,
// a reference's place in the file's references, then a base's place after every reference.
pub(crate) type Met = (usize, usize);

impl<'a> SymbolGraph<'a> {
    /// Resolve every reference, import and base class of the resolver's scan into edges.
    /// `deadline` is checked before each reference and base-class link.
    ///
    /// # Errors
    ///
    /// Returns the [`Stopped`] of the first check that stops the call.
    pub fn build(
        resolver: &Resolver<'_, 'a>,
        deadline: Option<&Deadline>,
    ) -> Result<SymbolGraph<'a>, Stopped> {
        let scopes = resolver.scopes();
        let nodes = Nodes::new(scopes.files(), true);
        let mut parts = Parts::new(&nodes);
        for (position, file) in scopes.files().iter().enumerate() {
            parts.import_edges(scopes, position);
            if in_name_graph(file) {
                parts.definitions_of(position);
                parts.symbol_links(resolver, position, None, deadline)?;
            }
        }
        Ok(parts.finish(scopes))
    }

    /// Build the module-level import edges alone, resolving no reference.
    #[must_use]
    pub fn imports(scopes: &Scopes<'a>) -> SymbolGraph<'a> {
        let nodes = Nodes::new(scopes.files(), false);
        let mut parts = Parts::new(&nodes);
        for position in 0..scopes.files().len() {
            parts.import_edges(scopes, position);
        }
        parts.finish(scopes)
    }

    /// Return every non-ambiguous edge, in sort-key order.
    #[must_use]
    pub fn edges(&self) -> &[SymbolEdge<'a>] {
        &self.edges
    }

    /// Return the edges leaving `node`, in sort-key order.
    #[must_use]
    pub fn outgoing(&self, node: &str) -> &[SymbolEdge<'a>] {
        let start = self.edges.partition_point(|edge| edge.source.node() < node);
        let end = self
            .edges
            .partition_point(|edge| edge.source.node() <= node);
        &self.edges[start..end]
    }

    /// Return the edges arriving at `node`, in sort-key order.
    pub fn incoming(&self, node: &str) -> impl Iterator<Item = &SymbolEdge<'a>> + '_ {
        let target = |at: &usize| self.edges[*at].target.node();
        let start = self.by_target.partition_point(|at| target(at) < node);
        let end = self.by_target.partition_point(|at| target(at) <= node);
        self.by_target[start..end].iter().map(|&at| &self.edges[at])
    }

    /// Return the module-level import edges, in sort-key order.
    pub fn import_edges(&self) -> impl Iterator<Item = &SymbolEdge<'a>> + '_ {
        self.edges
            .iter()
            .filter(|edge| edge.relation == Relation::Imports)
    }

    /// Return every symbol of the name-graph files, by stable id.
    #[must_use]
    pub fn definitions(&self) -> &BTreeMap<String, Definition<'a>> {
        &self.definitions
    }

    /// Return the scanned paths, in path order.
    #[must_use]
    pub fn files(&self) -> &[&'a str] {
        &self.files
    }

    /// Return whether `path` is a scanned file.
    #[must_use]
    pub fn has_file(&self, path: &str) -> bool {
        self.files.binary_search(&path).is_ok()
    }

    /// Return how many import statements named no file in this repository.
    #[must_use]
    pub fn unresolved_imports(&self) -> u32 {
        self.unresolved_imports
    }

    /// Return how many unresolved import statements named something this repository holds.
    #[must_use]
    pub fn unresolved_internal_imports(&self) -> u32 {
        self.unresolved_internal_imports
    }

    /// Return the ambiguous references, in sort-key order.
    #[must_use]
    pub fn ambiguous(&self) -> &[AmbiguousReference<'a>] {
        &self.ambiguous
    }
}

// Each file's node and each symbol's id and label, built at a file's first use and shared by
// every endpoint.
pub(crate) struct Nodes<'a> {
    files: &'a [&'a FileFacts],
    with_symbols: bool,
    paths: Vec<OnceLock<Arc<str>>>,
    symbols: Vec<OnceLock<Ids>>,
}

// Each symbol's id and label, in symbol order.
type Ids = Vec<(Arc<str>, Arc<str>)>;

impl<'a> Nodes<'a> {
    pub(crate) fn new(files: &'a [&'a FileFacts], with_symbols: bool) -> Nodes<'a> {
        Nodes {
            files,
            with_symbols,
            paths: files.iter().map(|_| OnceLock::new()).collect(),
            symbols: files.iter().map(|_| OnceLock::new()).collect(),
        }
    }

    fn path(&self, position: usize) -> &Arc<str> {
        self.paths[position].get_or_init(|| Arc::from(self.files[position].path.as_str()))
    }

    fn symbols(&self, position: usize) -> &Ids {
        self.symbols[position].get_or_init(|| {
            let file = self.files[position];
            let wanted = self.with_symbols && in_name_graph(file);
            let symbols = if wanted {
                file.facts.symbols.as_slice()
            } else {
                &[]
            };
            let ids = symbols.iter().map(|symbol| {
                let id = facts::StableId::of(file.language, &file.path, symbol).to_string();
                (Arc::from(id), Arc::from(symbol.qualname()))
            });
            ids.collect()
        })
    }

    pub(crate) fn file(&self, position: usize, line: u32) -> Endpoint<'a> {
        let node = Arc::clone(self.path(position));
        Endpoint {
            label: Arc::clone(&node),
            node,
            path: &self.files[position].path,
            line,
            is_symbol: false,
        }
    }

    pub(crate) fn symbol(&self, position: usize, symbol: usize) -> Endpoint<'a> {
        let (id, label) = &self.symbols(position)[symbol];
        Endpoint {
            node: Arc::clone(id),
            path: &self.files[position].path,
            line: self.files[position].facts.symbols[symbol].line,
            label: Arc::clone(label),
            is_symbol: true,
        }
    }

    pub(crate) fn definition(&self, definition: &Definition<'_>) -> Endpoint<'a> {
        let (file, symbol) = definition.position();
        self.symbol(file, symbol)
    }
}

// The occurrences of one file that an update links again: the references at `places`, ascending,
// and the bases of `names`, the names of those references.
#[derive(Default)]
pub(crate) struct Wanted<'a> {
    pub(crate) names: BTreeSet<&'a str>,
    pub(crate) places: Vec<usize>,
}

// One occurrence that a resolution links from: its endpoint, name, relation and place.
struct Link<'a> {
    source: Endpoint<'a>,
    name: &'a str,
    relation: Relation,
    met: Met,
}

// The links of one build or update, before they are sorted into a graph.
pub(crate) struct Parts<'n, 'a> {
    pub(crate) nodes: &'n Nodes<'a>,
    pub(crate) edges: Vec<SymbolEdge<'a>>,
    pub(crate) definitions: BTreeMap<String, Definition<'a>>,
    pub(crate) ambiguous: Vec<AmbiguousReference<'a>>,
    pub(crate) candidates: Vec<(&'a str, Vec<Endpoint<'a>>)>,
    pub(crate) met: Vec<Met>,
    pub(crate) candidate_names: BTreeMap<&'a str, usize>,
    pub(crate) bases: BTreeMap<&'a str, Vec<usize>>,
    // How many references and bases the links tried to resolve.
    pub(crate) sites: usize,
}

impl<'n, 'a> Parts<'n, 'a> {
    pub(crate) fn new(nodes: &'n Nodes<'a>) -> Parts<'n, 'a> {
        Parts {
            nodes,
            edges: Vec::new(),
            definitions: BTreeMap::new(),
            ambiguous: Vec::new(),
            candidates: Vec::new(),
            met: Vec::new(),
            candidate_names: BTreeMap::new(),
            bases: BTreeMap::new(),
            sites: 0,
        }
    }

    pub(crate) fn import_edges(&mut self, scopes: &Scopes<'a>, position: usize) {
        let nodes = self.nodes;
        let path = nodes.files[position].path.as_str();
        for (module, line, target) in scopes.at(position).resolved_edges(path) {
            let at = scopes
                .position(target)
                .expect("an import resolves only to a scanned file");
            let (source, target) = (nodes.file(position, line), nodes.file(at, 0));
            let name = Cow::Owned(module.to_owned());
            self.edges.extend(SymbolEdge::new(
                source,
                target,
                name,
                Relation::Imports,
                Tier::Imported,
            ));
        }
    }

    // Each symbol of the file by its id, and each base it names.
    pub(crate) fn definitions_of(&mut self, position: usize) {
        let nodes = self.nodes;
        let file = nodes.files[position];
        for (symbol, declared) in file.facts.symbols.iter().enumerate() {
            let definition = Definition::new(nodes.files, position, symbol);
            self.definitions
                .insert(nodes.symbol(position, symbol).node.to_string(), definition);
            let named = declared.bases.iter().map(|base| facts::base_name(base));
            for base in named.filter(|base| !base.is_empty()) {
                based(self.bases.entry(base).or_default(), position);
            }
        }
    }

    // Reference links first, then base-class links, as the candidate order depends on it. Only
    // the occurrences that `wanted` names, when given, are linked.
    pub(crate) fn symbol_links(
        &mut self,
        resolver: &Resolver<'_, 'a>,
        position: usize,
        wanted: Option<&Wanted<'_>>,
        deadline: Option<&Deadline>,
    ) -> Result<(), Stopped> {
        let file = self.nodes.files[position];
        let refs = &file.facts.refs;
        match wanted {
            None => self.reference_links(resolver, position, refs.iter().enumerate(), deadline)?,
            Some(wanted) => {
                let places = wanted.places.iter().map(|&place| (place, &refs[place]));
                self.reference_links(resolver, position, places, deadline)?;
            }
        }
        self.base_links(
            resolver,
            position,
            wanted.map(|wanted| &wanted.names),
            deadline,
        )
    }

    fn reference_links(
        &mut self,
        resolver: &Resolver<'_, 'a>,
        position: usize,
        refs: impl Iterator<Item = (usize, &'a Ref)>,
        deadline: Option<&Deadline>,
    ) -> Result<(), Stopped> {
        let nodes = self.nodes;
        let file = nodes.files[position];
        let owners = LineOwners::build(&file.facts.symbols);
        for (place, reference, resolution) in resolver.attempted(file, refs) {
            self.sites += 1;
            let Some(resolution) = resolution else {
                continue;
            };
            checkpoint(deadline)?;
            let source = match owners.owner_position(reference.line) {
                Some(owner) => nodes.symbol(position, owner),
                None => nodes.file(position, reference.line),
            };
            let link = Link {
                source,
                name: &reference.name,
                relation: Relation::References,
                met: (position, place),
            };
            self.link(link, &resolution);
        }
        Ok(())
    }

    fn base_links(
        &mut self,
        resolver: &Resolver<'_, 'a>,
        position: usize,
        names: Option<&BTreeSet<&str>>,
        deadline: Option<&Deadline>,
    ) -> Result<(), Stopped> {
        let nodes = self.nodes;
        let file = nodes.files[position];
        let after_refs = file.facts.refs.len();
        for (place, symbol, name) in bases(file) {
            if names.is_some_and(|names| !names.contains(name)) {
                continue;
            }
            self.sites += 1;
            let Some(resolution) = resolver.resolve(name, &file.path) else {
                continue;
            };
            checkpoint(deadline)?;
            let link = Link {
                source: nodes.symbol(position, symbol),
                name,
                relation: Relation::Inherits,
                met: (position, after_refs + place),
            };
            self.link(link, &resolution);
        }
        Ok(())
    }

    fn link(&mut self, link: Link<'a>, resolution: &Resolution<'a>) {
        let Link {
            source,
            name,
            relation,
            met,
        } = link;
        if resolution.tier != Tier::Ambiguous {
            for candidate in &resolution.candidates {
                let target = self.nodes.definition(candidate);
                let edge = SymbolEdge::new(
                    source.clone(),
                    target,
                    Cow::Borrowed(name),
                    relation,
                    resolution.tier,
                );
                self.edges.extend(edge);
            }
            return;
        }
        self.ambiguous.push(AmbiguousReference {
            source,
            name,
            relation,
        });
        if !self.candidate_names.contains_key(name) {
            self.candidate_names.insert(name, self.candidates.len());
            self.candidates
                .push((name, distinct_endpoints(self.nodes, resolution)));
            self.met.push(met);
        }
    }

    fn finish(mut self, scopes: &Scopes<'a>) -> SymbolGraph<'a> {
        self.edges.sort_by(SymbolEdge::cmp_total);
        self.edges.dedup();
        self.ambiguous.sort_by(AmbiguousReference::cmp_total);
        self.ambiguous.dedup();
        let mut by_target: Vec<usize> = (0..self.edges.len()).collect();
        by_target.sort_by(|left, right| {
            self.edges[*left]
                .target
                .node
                .cmp(&self.edges[*right].target.node)
        });
        let mut files: Vec<&'a str> = scopes
            .files()
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        files.sort_unstable();
        let mut graph = SymbolGraph {
            edges: self.edges,
            by_target,
            definitions: self.definitions,
            files,
            unresolved_imports: unresolved_imports(scopes),
            unresolved_internal_imports: unresolved_internal_imports(scopes),
            ambiguous: self.ambiguous,
            candidates: self.candidates,
            met: self.met,
            candidate_names: self.candidate_names,
            by_name: BTreeMap::new(),
            candidates_at: BTreeMap::new(),
            bases: self.bases,
        };
        graph.index_ambiguous();
        graph
    }
}

// A file's bases arrive together, so a new position is a new file.
fn based(files: &mut Vec<usize>, position: usize) {
    if files.last() != Some(&position) {
        files.push(position);
    }
}

// Each base name of the file's symbols, with its place among them and its symbol, in declaration
// order; an empty name names no class.
fn bases(file: &FileFacts) -> impl Iterator<Item = (usize, usize, &str)> {
    let declared = file.facts.symbols.iter().enumerate();
    let named = declared.flat_map(|(symbol, declared)| {
        let each = declared.bases.iter();
        each.map(move |base| (symbol, facts::base_name(base)))
    });
    let placed = named
        .enumerate()
        .map(|(place, (symbol, name))| (place, symbol, name));
    placed.filter(|(_, _, name)| !name.is_empty())
}

// The candidates' endpoints, each once, in candidate order.
pub(crate) fn distinct_endpoints<'a>(
    nodes: &Nodes<'a>,
    resolution: &Resolution<'_>,
) -> Vec<Endpoint<'a>> {
    let mut endpoints: Vec<Endpoint<'a>> = Vec::new();
    for endpoint in resolution
        .candidates
        .iter()
        .map(|candidate| nodes.definition(candidate))
    {
        if !endpoints.contains(&endpoint) {
            endpoints.push(endpoint);
        }
    }
    endpoints
}

#[cfg(test)]
mod canonical;
mod update;
