//! The symbols of the ranked files, scored, pinned and packed into the token budget.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use facts::{StableId, Symbol};
use graph::{Definition, FileFacts, NameIndex, common_name_damping, name_multiplier};

use super::{Listing, MapEntry, MapFile, Shortlist, count, map_rows_text};
use crate::budget::longest_fitting;
use crate::{TokenCounter, rationale_nodes};

// The focus-named symbols that the packing moves to the front, at most.
const FOCUS_PIN_LIMIT: usize = 10;
const AUTO_BUDGET_DIVISOR: usize = 6;
const AUTO_BUDGET_MIN: usize = 2_000;
const AUTO_BUDGET_MAX: usize = 8_000;
// Past this rendered size the clamped estimate is the maximum, whatever the rest of the set holds.
const AUTO_BUDGET_CEILING: usize = AUTO_BUDGET_MAX * AUTO_BUDGET_DIVISOR;
const AUTO_BUDGET_PROBE: usize = 2_000;
const AUTO_BUDGET_PROBE_GROWTH: usize = 4;

// What scores the symbols: the name index, the config's stoplist, and the focus's definitions.
pub(super) struct Symbols<'s, 'a> {
    pub(super) index: &'s NameIndex<'a>,
    pub(super) stoplist: &'s BTreeSet<String>,
    pub(super) definitions: &'s [Definition<'a>],
}

// One symbol competing for a place in the map.
#[derive(Clone)]
struct Candidate<'a> {
    score: f64,
    file: &'a FileFacts,
    symbol: &'a Symbol,
    stable_id: String,
}

// What the budget search renders: the symbols that compete, each file's count, and the ranking.
struct Packing<'p, 'a> {
    eligible: Vec<Candidate<'a>>,
    totals: BTreeMap<&'a str, usize>,
    ranked: &'p Shortlist<'p, 'a>,
}

// Only the files the focus reaches compete for the budget; every ranked file is still listed.
pub(super) fn symbol_listing(
    counter: &dyn TokenCounter,
    ranked: &Shortlist<'_, '_>,
    symbols: &Symbols<'_, '_>,
    budget: Option<usize>,
) -> Listing {
    let candidates = score_symbols(ranked, symbols);
    let focus_order = focus_order(symbols.definitions, ranked.chosen);
    let mut totals = BTreeMap::new();
    for candidate in &candidates {
        *totals.entry(candidate.file.path.as_str()).or_insert(0) += 1;
    }
    let reached = candidates
        .into_iter()
        .filter(|c| ranked.ranking.reached(&c.file.path));
    let packing = Packing {
        eligible: pinned_first(reached.collect(), &focus_order),
        totals,
        ranked,
    };
    let budget = budget.unwrap_or_else(|| auto_budget(counter, &packing));
    let included = pack(counter, &packing, budget);
    let kept = &packing.eligible[..included];
    Listing {
        files: group(included, &packing),
        budget,
        included,
        candidates: packing.eligible.len(),
        expand_order: kept.iter().map(|c| c.stable_id.clone()).collect(),
        focus_order,
    }
}

// A file's rank spread across its symbols by inbound name references, damped as the edges are.
fn score_symbols<'a>(ranked: &Shortlist<'_, 'a>, symbols: &Symbols<'_, '_>) -> Vec<Candidate<'a>> {
    let mut candidates = Vec::new();
    for &path in ranked.chosen {
        let file = ranked.file(path);
        let file_rank = ranked.rank(path);
        candidates.extend(file.facts.symbols.iter().map(|symbol| Candidate {
            score: file_rank * (1.0 + inbound_weight(path, &symbol.name, symbols)),
            file,
            symbol,
            stable_id: StableId::of(file.language, path, symbol).to_string(),
        }));
    }
    candidates.sort_by(|a, b| {
        let score = b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal);
        let path = || a.file.path.cmp(&b.file.path);
        score
            .then_with(path)
            .then(a.symbol.line.cmp(&b.symbol.line))
    });
    candidates
}

fn inbound_weight(path: &str, name: &str, symbols: &Symbols<'_, '_>) -> f64 {
    let sites = symbols.index.sites(name);
    let inbound = sites.iter().filter(|site| site.path() != path).count();
    let spread = symbols.index.spelling_spread(name);
    count(inbound) * (name_multiplier(name, symbols.stoplist) / common_name_damping(spread))
}

// The focus-named symbols in the chosen files, in file order then line, each id once.
fn focus_order(definitions: &[Definition<'_>], chosen: &[&str]) -> Vec<String> {
    let position: BTreeMap<&str, usize> = (chosen.iter().enumerate())
        .map(|(at, path)| (*path, at))
        .collect();
    let mut placed: Vec<(usize, u32, String)> = (definitions.iter())
        .filter_map(|definition| {
            let at = *position.get(definition.path())?;
            Some((
                at,
                definition.symbol().line,
                definition.stable_id().to_string(),
            ))
        })
        .collect();
    placed.sort_by_key(|&(at, line, _)| (at, line));
    let mut seen = BTreeSet::new();
    (placed.into_iter())
        .map(|(_, _, id)| id)
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

// The packing keeps a prefix, so a named symbol moved to the front is a named symbol kept.
fn pinned_first<'a>(eligible: Vec<Candidate<'a>>, focus_order: &[String]) -> Vec<Candidate<'a>> {
    let position: BTreeMap<&str, usize> = (focus_order.iter().take(FOCUS_PIN_LIMIT).enumerate())
        .map(|(at, id)| (id.as_str(), at))
        .collect();
    let (mut pinned, rest): (Vec<_>, Vec<_>) = (eligible.into_iter())
        .partition(|candidate| position.contains_key(candidate.stable_id.as_str()));
    pinned.sort_by_key(|candidate| position[candidate.stable_id.as_str()]);
    pinned.extend(rest);
    pinned
}

// The first `shown` eligible symbols, grouped under every ranked file in rank order.
fn group(shown: usize, packing: &Packing<'_, '_>) -> Vec<MapFile> {
    let shown = shown.min(packing.eligible.len());
    let mut per_file: BTreeMap<&str, Vec<&Candidate<'_>>> = BTreeMap::new();
    for candidate in &packing.eligible[..shown] {
        per_file
            .entry(candidate.file.path.as_str())
            .or_default()
            .push(candidate);
    }
    let ranked = packing.ranked;
    let files = ranked.chosen.iter().map(|&path| {
        let mut chosen = per_file.remove(path).unwrap_or_default();
        chosen.sort_by_key(|candidate| candidate.symbol.line);
        let prefix = chosen
            .first()
            .map(|first| first.file.language.extraction().prefix);
        MapFile {
            path: path.to_owned(),
            rank: ranked.rank(path),
            entries: chosen.into_iter().map(entry).collect(),
            total: packing.totals.get(path).copied().unwrap_or(0),
            id_prefix: prefix.unwrap_or_default(),
            reached: ranked.ranking.reached(path),
            commits_window: None,
            last_commit_days: None,
        }
    });
    files.collect()
}

fn entry(candidate: &Candidate<'_>) -> MapEntry {
    let (file, symbol) = (candidate.file, candidate.symbol);
    MapEntry {
        line: symbol.line,
        signature: symbol.signature.clone(),
        stable_id: candidate.stable_id.clone(),
        depth: usize::from(!symbol.parent.is_empty()),
        rationales: rationale_nodes(file.language, &file.path, symbol),
        name: symbol.id_qualname(),
    }
}

// Rendered in growing prefixes: once a prefix renders past the ceiling the answer is known.
fn auto_budget(counter: &dyn TokenCounter, packing: &Packing<'_, '_>) -> usize {
    let mut size = AUTO_BUDGET_PROBE;
    loop {
        let rendered = counter.count(&map_rows_text(&group(size, packing)));
        if rendered > AUTO_BUDGET_CEILING {
            return AUTO_BUDGET_MAX;
        }
        if size >= packing.eligible.len() {
            return (rendered / AUTO_BUDGET_DIVISOR).clamp(AUTO_BUDGET_MIN, AUTO_BUDGET_MAX);
        }
        size *= AUTO_BUDGET_PROBE_GROWTH;
    }
}

// The largest prefix of the eligible symbols whose render, file rows included, fits `budget`.
fn pack(counter: &dyn TokenCounter, packing: &Packing<'_, '_>, budget: usize) -> usize {
    let fits = |kept: usize| counter.count(&map_rows_text(&group(kept, packing))) <= budget;
    longest_fitting(0, packing.eligible.len(), fits)
}
