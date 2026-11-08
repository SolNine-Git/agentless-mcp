//! A symbol's card: callers, tests and similar functions from refs, callees from explain, and the
//! last change from history, or one section in full. Each count is a total that the operation
//! took before its cut, or a [`Tally::Floor`] when the list it counts was cut.

use std::collections::BTreeSet;

use graph::Tier;

use super::change::last_change;
use super::{
    Callees, Callers, EdgeGroup, LAST_CHANGE_DEADLINE, SYMBOL_ROWS, Section, Similar, SymbolTrace,
    Tally, Tests, TierSites, TierTotal, cap,
};
use crate::ops::limits::{DEFAULT_HISTORY_BUDGET, DEFAULT_HISTORY_LIMIT, MAX_LIMIT};
use crate::{
    Error, ExpandRequest, ExplainRequest, Explanation, HistoryRequest, HistoryResult, IntArg,
    Noted, Reading, RefGroup, RefsRequest, RefsResult, Services, SharedCaller, SymbolCard,
    TierGroup, expand, explain, history, is_test_path, refs,
};

// The sections that a default card holds and that `more` can also name alone.
const DEFAULT_SECTIONS: [Section; 4] = [
    Section::Callers,
    Section::Tests,
    Section::Similar,
    Section::Callees,
];

pub(super) fn symbol_trace(
    services: &Services,
    reading: &Reading<'_>,
    card: SymbolCard,
    more: Option<Section>,
) -> Noted<Result<SymbolTrace, Error>> {
    let sections = Sections {
        services,
        reading,
        id: card.stable_id.clone(),
        more,
    };
    let empty = SymbolTrace {
        card,
        callers: None,
        callees: None,
        tests: None,
        similar: None,
        change: None,
        history: None,
        body: None,
    };
    let traced = sections.with_refs(empty);
    let traced = traced.then_ok(|traced| sections.with_explain(traced));
    traced.then_ok(|traced| sections.with_git(traced))
}

struct Sections<'s, 'r> {
    services: &'s Services,
    reading: &'s Reading<'r>,
    id: String,
    more: Option<Section>,
}

impl Sections<'_, '_> {
    fn wants(&self, section: Section) -> bool {
        match self.more {
            None => DEFAULT_SECTIONS.contains(&section),
            Some(asked) => asked == section,
        }
    }

    // One refs call fills callers, tests and similar; it ranks the shared callers only when the
    // card shows them.
    fn with_refs(&self, traced: SymbolTrace) -> Noted<Result<SymbolTrace, Error>> {
        let sections = [Section::Callers, Section::Tests, Section::Similar];
        if !sections.iter().any(|section| self.wants(*section)) {
            return Noted::new(Ok(traced));
        }
        let request = RefsRequest {
            target: self.id.clone(),
            limit: IntArg::from_usize(MAX_LIMIT),
            shared_callers: self.wants(Section::Similar),
        };
        let found = refs(self.services, self.reading, &request);
        found.map_ok(|found| SymbolTrace {
            callers: (self.wants(Section::Callers))
                .then(|| callers(&found, cap(self.more, Section::Callers, SYMBOL_ROWS))),
            tests: (self.wants(Section::Tests))
                .then(|| tests(&found, cap(self.more, Section::Tests, SYMBOL_ROWS))),
            similar: (self.wants(Section::Similar))
                .then(|| similar(&found, cap(self.more, Section::Similar, SYMBOL_ROWS))),
            ..traced
        })
    }

    fn with_explain(&self, traced: SymbolTrace) -> Noted<Result<SymbolTrace, Error>> {
        if !self.wants(Section::Callees) {
            return Noted::new(Ok(traced));
        }
        let request = ExplainRequest {
            target: self.id.clone(),
            limit: IntArg::from_usize(MAX_LIMIT),
        };
        let explained = explain(self.services, self.reading, &request);
        explained.map_ok(|explained| SymbolTrace {
            callees: Some(callees(
                &explained,
                cap(self.more, Section::Callees, SYMBOL_ROWS),
            )),
            ..traced
        })
    }

    // A default card waits for its last change under the deadline; `more` reads the history or
    // the body in full.
    fn with_git(&self, traced: SymbolTrace) -> Noted<Result<SymbolTrace, Error>> {
        match self.more {
            None => {
                let change =
                    last_change(self.services, self.reading, &self.id, LAST_CHANGE_DEADLINE);
                Noted::new(Ok(SymbolTrace {
                    change: Some(change),
                    ..traced
                }))
            }
            Some(Section::History) => self.history().map(|history| {
                history.map(|history| SymbolTrace {
                    history: Some(history),
                    ..traced
                })
            }),
            Some(Section::Body) => self.body().map(|body| {
                body.map(|body| SymbolTrace {
                    body: Some(body),
                    ..traced
                })
            }),
            Some(_) => Noted::new(Ok(traced)),
        }
    }

    fn history(&self) -> Noted<Result<Result<HistoryResult, String>, Error>> {
        let request = HistoryRequest {
            target: self.id.clone(),
            limit: IntArg::from_usize(DEFAULT_HISTORY_LIMIT),
            budget: IntArg::from_usize(DEFAULT_HISTORY_BUDGET),
        };
        history(self.services, self.reading, &request).map(refusal_text)
    }

    fn body(&self) -> Noted<Result<Result<SymbolCard, String>, Error>> {
        let request = ExpandRequest {
            ids: vec![self.id.clone()],
            limit: IntArg::from_usize(1),
        };
        let expanded = expand(self.services, self.reading, &request);
        expanded.map_ok(|mut expanded| match expanded.cards.pop() {
            Some(card) => Ok(card),
            None => Err((expanded.unresolved.into_iter())
                .map(|(_, reason)| reason)
                .collect::<Vec<_>>()
                .join("; ")),
        })
    }
}

// A stopped call stops the trace; any other refusal is the section's text.
fn refusal_text<T>(outcome: Result<T, Error>) -> Result<Result<T, String>, Error> {
    match outcome {
        Ok(value) => Ok(Ok(value)),
        Err(error @ Error::Kind(kernel::Error::CallStopped(_))) => Err(error),
        Err(error) => Ok(Err(error.to_string())),
    }
}

fn binds(tier: &str) -> bool {
    (Tier::ORDER.iter()).any(|known| known.as_str() == tier && known.binds())
}

fn callers(found: &RefsResult, cap: usize) -> Callers {
    let listing = &found.groups;
    let whole = listing.omitted() == 0;
    let binding: Vec<&RefGroup> = (listing.rows.iter())
        .filter(|group| binds(group.tier) && !is_test_path(&group.path))
        .collect();
    let name_only: BTreeSet<&str> = (listing.rows.iter())
        .filter(|group| !binds(group.tier))
        .map(|group| group.path.as_str())
        .collect();
    Callers {
        bound: listing.bound,
        name_only: listing.total - listing.bound,
        name_only_files: Tally::of(name_only.len(), whole),
        files: listing.files,
        tiers: whole.then(|| tier_sites(&listing.rows)),
        rows: binding
            .iter()
            .take(cap)
            .map(|group| (*group).clone())
            .collect(),
        rows_total: Tally::of(binding.len(), whole),
        coverage: found.coverage,
    }
}

// A group is one file at one tier, so a tier's groups are its files.
fn tier_sites(groups: &[RefGroup]) -> Vec<TierSites> {
    let of_tier = |tier: Tier| (groups.iter()).filter(move |group| group.tier == tier.as_str());
    (Tier::ORDER.into_iter())
        .map(|tier| TierSites {
            tier: tier.as_str(),
            sites: of_tier(tier).map(|group| group.sites.len()).sum(),
            files: of_tier(tier).count(),
        })
        .filter(|tier| tier.files > 0)
        .collect()
}

fn tests(found: &RefsResult, cap: usize) -> Tests {
    let listing = &found.groups;
    let tested: Vec<&RefGroup> = (listing.rows.iter())
        .filter(|group| is_test_path(&group.path) && group.tier != Tier::Ambiguous.as_str())
        .collect();
    Tests {
        rows: tested
            .iter()
            .take(cap)
            .map(|group| (*group).clone())
            .collect(),
        total: Tally::of(tested.len(), listing.omitted() == 0),
    }
}

// A candidate ranks through a bound caller when one of its shared callers holds a binding-tier
// site of the target.
fn similar(found: &RefsResult, cap: usize) -> Similar {
    let listing = &found.groups;
    let bound: BTreeSet<(&str, &str)> = (listing.rows.iter())
        .filter(|group| binds(group.tier))
        .flat_map(|group| {
            let path = group.path.as_str();
            group
                .sites
                .iter()
                .map(move |site| (path, site.enclosing.as_str()))
        })
        .collect();
    let through_bound = |row: &&SharedCaller| {
        (row.callers.iter())
            .any(|caller| bound.contains(&(caller.path.as_str(), caller.qualname.as_str())))
    };
    let shared = &found.shared;
    let ranked: Vec<&SharedCaller> = shared.rows().iter().filter(through_bound).collect();
    let whole = listing.omitted() == 0 && shared.rows().len() == shared.total();
    Similar {
        total: shared.total(),
        rows: ranked.iter().take(cap).map(|row| (*row).clone()).collect(),
        bound_total: Tally::of(ranked.len(), whole),
    }
}

fn callees(explained: &Explanation, cap: usize) -> Callees {
    let tiers = (explained.fan_out.iter())
        .map(|group| TierTotal {
            tier: group.tier,
            total: group.total,
        })
        .collect();
    let binding: Vec<&TierGroup> = (explained.fan_out.iter())
        .filter(|group| binds(group.tier))
        .collect();
    let bound: usize = binding.iter().map(|group| group.total).sum();
    let all: usize = explained.fan_out.iter().map(|group| group.total).sum();
    let whole = binding.iter().all(|group| group.rows.len() == group.total);
    let groups: Vec<EdgeGroup> = binding.into_iter().flat_map(by_file).collect();
    Callees {
        bound,
        name_only: all - bound,
        rows_total: Tally::of(groups.len(), whole),
        rows: groups.into_iter().take(cap).collect(),
        tiers,
    }
}

// One tier's edges by the file of their other end, each file at its first edge's place.
fn by_file(group: &TierGroup) -> Vec<EdgeGroup> {
    let mut files: Vec<EdgeGroup> = Vec::new();
    for row in &group.rows {
        match files.iter_mut().find(|file| file.path == row.path) {
            Some(file) => file.edges.push(row.clone()),
            None => files.push(EdgeGroup {
                path: row.path.clone(),
                tier: group.tier,
                edges: vec![row.clone()],
            }),
        }
    }
    files
}
