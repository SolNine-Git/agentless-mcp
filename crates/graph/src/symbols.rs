//! The symbol graph: every resolved reference, import and base class of one scan, as edges
//! between symbols and files, each with the evidence tier behind it.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

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
    pub(crate) candidate_names: BTreeMap<&'a str, usize>,
    pub(crate) by_name: BTreeMap<&'a str, Vec<usize>>,
    pub(crate) candidates_at: BTreeMap<Arc<str>, Vec<(&'a str, Endpoint<'a>)>>,
}

impl<'a> SymbolGraph<'a> {
    /// Resolve every reference, import and base class of the resolver's scan into edges.
    #[must_use]
    pub fn build(resolver: &Resolver<'a>) -> SymbolGraph<'a> {
        let scopes = resolver.scopes();
        let nodes = Nodes::build(scopes.files(), true);
        let mut parts = Parts::new(&nodes);
        for (position, file) in scopes.files().iter().enumerate() {
            parts.import_edges(scopes, position);
            if in_name_graph(file) {
                parts.symbol_links(resolver, position);
            }
        }
        parts.finish(scopes)
    }

    /// Build the module-level import edges alone, resolving no reference.
    #[must_use]
    pub fn imports(scopes: &Scopes<'a>) -> SymbolGraph<'a> {
        let nodes = Nodes::build(scopes.files(), false);
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

// Each file's node and each symbol's id and label, built once and shared by every endpoint.
struct Nodes<'a> {
    files: &'a [FileFacts],
    paths: Vec<Arc<str>>,
    symbols: Vec<Vec<(Arc<str>, Arc<str>)>>,
}

impl<'a> Nodes<'a> {
    fn build(files: &'a [FileFacts], with_symbols: bool) -> Nodes<'a> {
        let symbols = files
            .iter()
            .map(|file| {
                let wanted = with_symbols && in_name_graph(file);
                let symbols = if wanted {
                    file.facts.symbols.as_slice()
                } else {
                    &[]
                };
                symbols
                    .iter()
                    .map(|symbol| {
                        let id = facts::StableId::of(file.language, &file.path, symbol).to_string();
                        (Arc::from(id), Arc::from(symbol.qualname()))
                    })
                    .collect()
            })
            .collect();
        Nodes {
            files,
            paths: files
                .iter()
                .map(|file| Arc::from(file.path.as_str()))
                .collect(),
            symbols,
        }
    }

    fn file(&self, position: usize, line: u32) -> Endpoint<'a> {
        let node = Arc::clone(&self.paths[position]);
        Endpoint {
            label: Arc::clone(&node),
            node,
            path: &self.files[position].path,
            line,
            is_symbol: false,
        }
    }

    fn symbol(&self, position: usize, symbol: usize) -> Endpoint<'a> {
        let (id, label) = &self.symbols[position][symbol];
        Endpoint {
            node: Arc::clone(id),
            path: &self.files[position].path,
            line: self.files[position].facts.symbols[symbol].line,
            label: Arc::clone(label),
            is_symbol: true,
        }
    }

    fn definition(&self, definition: &Definition<'a>) -> Endpoint<'a> {
        let (file, symbol) = definition.position();
        self.symbol(file, symbol)
    }
}

struct Parts<'n, 'a> {
    nodes: &'n Nodes<'a>,
    edges: Vec<SymbolEdge<'a>>,
    definitions: BTreeMap<String, Definition<'a>>,
    ambiguous: Vec<AmbiguousReference<'a>>,
    candidates: Vec<(&'a str, Vec<Endpoint<'a>>)>,
    candidate_names: BTreeMap<&'a str, usize>,
}

impl<'n, 'a> Parts<'n, 'a> {
    fn new(nodes: &'n Nodes<'a>) -> Parts<'n, 'a> {
        Parts {
            nodes,
            edges: Vec::new(),
            definitions: BTreeMap::new(),
            ambiguous: Vec::new(),
            candidates: Vec::new(),
            candidate_names: BTreeMap::new(),
        }
    }

    fn import_edges(&mut self, scopes: &Scopes<'a>, position: usize) {
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

    // Reference links first, then base-class links, as the candidate order depends on it.
    fn symbol_links(&mut self, resolver: &Resolver<'a>, position: usize) {
        let nodes = self.nodes;
        let file = &nodes.files[position];
        for (symbol, _) in file.facts.symbols.iter().enumerate() {
            let definition = Definition::new(nodes.files, position, symbol);
            self.definitions
                .insert(nodes.symbol(position, symbol).node.to_string(), definition);
        }
        let owners = LineOwners::build(&file.facts.symbols);
        for (reference, resolution) in resolver.resolved_sites(file, None) {
            let source = match owners.owner_position(reference.line) {
                Some(owner) => nodes.symbol(position, owner),
                None => nodes.file(position, reference.line),
            };
            self.link(source, &reference.name, &resolution, Relation::References);
        }
        for (symbol, name, resolution) in inheritance(resolver, file) {
            self.link(
                nodes.symbol(position, symbol),
                name,
                &resolution,
                Relation::Inherits,
            );
        }
    }

    fn link(
        &mut self,
        source: Endpoint<'a>,
        name: &'a str,
        resolution: &Resolution<'a>,
        relation: Relation,
    ) {
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
            candidate_names: self.candidate_names,
            by_name: BTreeMap::new(),
            candidates_at: BTreeMap::new(),
        };
        graph.index_ambiguous();
        graph
    }
}

// Each (symbol, base name, resolution) of the file's declared base classes, in declaration order.
fn inheritance<'a>(
    resolver: &Resolver<'a>,
    file: &'a FileFacts,
) -> Vec<(usize, &'a str, Resolution<'a>)> {
    let bases = file
        .facts
        .symbols
        .iter()
        .enumerate()
        .flat_map(|(symbol, declared)| {
            declared
                .bases
                .iter()
                .map(move |base| (symbol, facts::base_name(base)))
        });
    bases
        .filter(|(_, name)| !name.is_empty())
        .filter_map(|(symbol, name)| Some((symbol, name, resolver.resolve(name, &file.path)?)))
        .collect()
}

// The candidates' endpoints, each once, in candidate order.
fn distinct_endpoints<'a>(nodes: &Nodes<'a>, resolution: &Resolution<'a>) -> Vec<Endpoint<'a>> {
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
