//! Structural health: orphan candidates, unused exports and hubs over the resolved symbol graph,
//! with the complexity summary and the hotspots of one budgeted metrics walk.

mod complexity;

use std::collections::BTreeMap;

use facts::SymbolKind;
use graph::{Definition, SymbolGraph, Tier};
use kernel::Bounded;

use crate::bounds::within;
use crate::endpoints::{tier_at, with_resolved};
use crate::ops::limits::MAX_LIMIT;
use crate::scan::RepoFacts;
use crate::{Error, IntArg, Noted, Reading, Services, is_fixture_path, is_test_path};

pub use complexity::{HealthComplexity, HealthComplexityRow};

// Every method would be a permanent orphan, because a call through a selector resolves to no
// edge, so only functions can be orphans.
const ORPHAN_KINDS: [SymbolKind; 1] = [SymbolKind::Function];
const HUB_KINDS: [SymbolKind; 6] = [
    SymbolKind::Function,
    SymbolKind::Method,
    SymbolKind::Class,
    SymbolKind::Dataclass,
    SymbolKind::Protocol,
    SymbolKind::Enum,
];

/// A health request: how many rows each section lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRequest {
    /// The most rows per section. Every count is complete.
    pub limit: IntArg,
}

/// The three graph sections and the complexity sections over one repository.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthReport {
    /// Functions with no counted edge in or out.
    pub orphans: Bounded<HealthSymbol>,
    /// Public functions with no counted edge in.
    pub unused_exports: Bounded<HealthSymbol>,
    /// Functions, methods and classes with a counted edge, highest counted degree first.
    pub hubs: Bounded<HealthSymbol>,
    /// The complexity summary of the walk.
    pub complexity: HealthComplexity,
    /// The measured units with the most mass, highest first.
    pub hotspots: Bounded<HealthComplexityRow>,
    /// How many definitions the graph sections cover.
    pub symbols: usize,
    /// How many definitions the test and fixture rule left out.
    pub excluded: usize,
}

/// One symbol in a graph section, with the degree that placed it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthSymbol {
    /// The stable id.
    pub stable_id: String,
    /// The path below the repository root.
    pub path: String,
    /// The first line.
    pub line: u32,
    /// The qualified name.
    pub label: String,
    /// The kind's name.
    pub kind: &'static str,
    /// The same-file and resolved-via-import edges that arrive.
    pub in_degree: usize,
    /// The same-file and resolved-via-import edges that leave.
    pub out_degree: usize,
    /// The weak matches the degree left out, per tier, strongest tier first.
    pub discounted: Vec<DiscountedTier>,
}

/// How many matches of one weak tier a degree left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscountedTier {
    /// The tier's label.
    pub tier: &'static str,
    /// The matches of that tier.
    pub count: usize,
}

/// Reports the health sections of the repository that `reading` names, `limit` rows each. The
/// scan's and the walk's warnings come with either outcome.
#[must_use]
pub fn health(
    services: &Services,
    reading: &Reading<'_>,
    request: &HealthRequest,
) -> Noted<Result<HealthReport, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    let Noted {
        value,
        mut diagnostics,
    } = RepoFacts::load(services, reading);
    let config = &reading.repo.config;
    let threshold = config
        .complexity_threshold
        .unwrap_or(metrics::DEFAULT_THRESHOLD);
    let value = value.map(|facts| {
        let walk = complexity::walk(services, reading, &facts);
        diagnostics.extend(walk.diagnostics);
        with_resolved(&facts.files, |_, graph| {
            report(graph, &walk.value, threshold, limit)
        })
    });
    Noted { value, diagnostics }
}

fn report(
    graph: &SymbolGraph<'_>,
    walk: &complexity::Walk,
    threshold: u32,
    limit: usize,
) -> HealthReport {
    let degrees = degrees(graph);
    let considered: Vec<(&str, &Definition<'_>)> = (graph.definitions().iter())
        .filter(|(_, definition)| in_health_scope(definition.path()))
        .map(|(id, definition)| (id.as_str(), definition))
        .collect();
    let functions = of_kinds(&considered, &degrees, &ORPHAN_KINDS);
    let orphans: Vec<&Entry<'_, '_>> = (functions.iter())
        .filter(|entry| entry.degree.counted() == 0)
        .collect();
    let unused: Vec<&Entry<'_, '_>> = (functions.iter())
        .filter(|entry| entry.definition.symbol().is_public && entry.degree.arriving.counted == 0)
        .collect();
    let mut hubs: Vec<Entry<'_, '_>> = of_kinds(&considered, &degrees, &HUB_KINDS);
    hubs.retain(|entry| entry.degree.counted() > 0);
    hubs.sort_by_key(|entry| {
        let symbol = entry.definition.symbol();
        (
            std::cmp::Reverse(entry.degree.counted()),
            entry.definition.path(),
            symbol.line,
        )
    });
    HealthReport {
        orphans: section(orphans, limit, false),
        unused_exports: section(unused, limit, true),
        hubs: section(hubs.iter().collect(), limit, false),
        complexity: complexity::summary(walk, threshold, limit),
        hotspots: complexity::hotspots(&walk.units, graph.definitions(), limit),
        symbols: considered.len(),
        excluded: graph.definitions().len() - considered.len(),
    }
}

// A fixture exists to be parsed and a test helper is called by a runner, so both would be
// permanent orphans.
fn in_health_scope(path: &str) -> bool {
    !is_test_path(path) && !is_fixture_path(path)
}

// One symbol's edges at each end.
#[derive(Clone, Copy, Default)]
struct Degree {
    arriving: End,
    leaving: End,
}

impl Degree {
    fn counted(&self) -> usize {
        self.arriving.counted + self.leaving.counted
    }
}

// The counted edges at one end, and the weak matches per tier that were not counted.
#[derive(Clone, Copy, Default)]
struct End {
    counted: usize,
    discounted: [usize; 4],
}

impl End {
    // Same-file and resolved-via-import edges are bindings and count; a weaker match is kept
    // apart by tier.
    fn add(&mut self, tier: Tier) {
        if matches!(tier, Tier::SameFile | Tier::Imported) {
            self.counted += 1;
        } else {
            self.discounted[tier_at(tier)] += 1;
        }
    }
}

// The side of a degree that one count of ambiguous edges lands on.
type Side = fn(&mut Degree) -> &mut End;

struct Entry<'d, 'a> {
    stable_id: &'d str,
    definition: &'d Definition<'a>,
    degree: Degree,
}

fn degrees(graph: &SymbolGraph<'_>) -> BTreeMap<String, Degree> {
    let mut degrees: BTreeMap<String, Degree> = BTreeMap::new();
    for edge in graph.edges() {
        let source = degrees.entry(edge.source().node().to_owned()).or_default();
        source.leaving.add(edge.tier());
        let target = degrees.entry(edge.target().node().to_owned()).or_default();
        target.arriving.add(edge.tier());
    }
    let ambiguous = graph.ambiguous_degrees();
    let at = tier_at(Tier::Ambiguous);
    let sides: [(&BTreeMap<String, u64>, Side); 2] = [
        (&ambiguous.arriving, |degree| &mut degree.arriving),
        (&ambiguous.leaving, |degree| &mut degree.leaving),
    ];
    for (counts, side) in sides {
        for (node, count) in counts {
            side(degrees.entry(node.clone()).or_default()).discounted[at] += as_count(*count);
        }
    }
    degrees
}

fn as_count(count: u64) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

// The definitions of `kinds`, by path, line and stable id: two symbols can start on one line.
fn of_kinds<'d, 'a>(
    considered: &[(&'d str, &'d Definition<'a>)],
    degrees: &BTreeMap<String, Degree>,
    kinds: &[SymbolKind],
) -> Vec<Entry<'d, 'a>> {
    let mut entries: Vec<Entry<'d, 'a>> = (considered.iter())
        .filter(|(_, definition)| kinds.contains(&definition.symbol().kind))
        .map(|&(stable_id, definition)| Entry {
            stable_id,
            definition,
            degree: degrees.get(stable_id).copied().unwrap_or_default(),
        })
        .collect();
    entries.sort_by_key(|entry| {
        let line = entry.definition.symbol().line;
        (entry.definition.path(), line, entry.stable_id)
    });
    entries
}

// An unused export is a claim about what reaches the symbol, so its row names only the weak
// matches that arrive.
fn section(
    entries: Vec<&Entry<'_, '_>>,
    limit: usize,
    inbound_only: bool,
) -> Bounded<HealthSymbol> {
    let rows = entries
        .into_iter()
        .map(|entry| row(entry, inbound_only))
        .collect();
    Bounded::cut(rows, limit)
}

fn row(entry: &Entry<'_, '_>, inbound_only: bool) -> HealthSymbol {
    let degree = &entry.degree;
    let symbol = entry.definition.symbol();
    let discounted = (Tier::ORDER.iter().enumerate())
        .map(|(at, tier)| {
            let outbound = if inbound_only {
                0
            } else {
                degree.leaving.discounted[at]
            };
            (tier, degree.arriving.discounted[at] + outbound)
        })
        .filter(|(_, count)| *count > 0)
        .map(|(tier, count)| DiscountedTier {
            tier: tier.label(),
            count,
        })
        .collect();
    HealthSymbol {
        stable_id: entry.stable_id.to_owned(),
        path: entry.definition.path().to_owned(),
        line: symbol.line,
        label: symbol.qualname(),
        kind: symbol.kind.as_str(),
        in_degree: degree.arriving.counted,
        out_degree: degree.leaving.counted,
        discounted,
    }
}

#[cfg(test)]
mod tests;
