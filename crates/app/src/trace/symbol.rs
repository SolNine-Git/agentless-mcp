//! A symbol's card: callers and tests from refs, callees from explain, and the last change from
//! history, or one section in full, such as the similar functions from refs. Each count is a total
//! that the operation took before its cut, or a [`Tally::Floor`] when the list it counts was cut.

use std::collections::BTreeSet;

use graph::Tier;

use super::change::last_change;
use super::rev;
use super::{
    Call, Callees, Callers, EdgeGroup, LAST_CHANGE_DEADLINE, LISTING_LIMIT, SYMBOL_ROWS, Section,
    Similar, SimilarRow, SymbolTrace, Tally, Tests, TierSites, TierTotal, cap,
};
use crate::explain::explain_in;
use crate::ops::limits::{DEFAULT_HISTORY_BUDGET, DEFAULT_HISTORY_LIMIT};
use crate::refs::{binds, refs_in};
use crate::{
    Error, ExpandRequest, ExplainRequest, Explanation, HistoryRequest, HistoryResult, IntArg,
    Noted, RefGroup, RefsRequest, RefsResult, SharedCaller, SymbolCard, TierGroup, expand, history,
    is_test_path,
};

// The sections that a default card holds and that `more` can also name alone.
const DEFAULT_SECTIONS: [Section; 3] = [Section::Callers, Section::Tests, Section::Callees];

pub(super) fn symbol_trace(
    call: &Call<'_, '_, '_, '_>,
    card: SymbolCard,
    more: Option<Section>,
) -> Noted<Result<SymbolTrace, Error>> {
    let sections = Sections {
        call,
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

struct Sections<'s, 'c, 'r, 'g, 'f> {
    call: &'s Call<'c, 'r, 'g, 'f>,
    id: String,
    more: Option<Section>,
}

impl Sections<'_, '_, '_, '_, '_> {
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
            limit: IntArg::from_usize(LISTING_LIMIT),
            shared_callers: self.wants(Section::Similar),
        };
        let call = self.call;
        let found = refs_in(call.reading, &request, call.graph, call.scan);
        found.map_ok(|found| SymbolTrace {
            callers: (self.wants(Section::Callers)).then(|| callers(&found, self.more)),
            tests: (self.wants(Section::Tests))
                .then(|| tests(&found, cap(self.more, Section::Tests, SYMBOL_ROWS))),
            similar: (self.wants(Section::Similar)).then(|| similar(&found)),
            ..traced
        })
    }

    fn with_explain(&self, traced: SymbolTrace) -> Noted<Result<SymbolTrace, Error>> {
        if !self.wants(Section::Callees) {
            return Noted::new(Ok(traced));
        }
        let request = ExplainRequest {
            target: self.id.clone(),
            limit: IntArg::from_usize(LISTING_LIMIT),
        };
        let call = self.call;
        let explained = explain_in(call.reading, &request, call.graph, &call.scan.skipped);
        explained.map_ok(|explained| SymbolTrace {
            callees: Some(callees(&explained, self.more)),
            ..traced
        })
    }

    // A default card waits for its last change under the deadline; `more` reads the history or
    // the body in full.
    fn with_git(&self, traced: SymbolTrace) -> Noted<Result<SymbolTrace, Error>> {
        match self.more {
            None => {
                let call = self.call;
                let at = call.tree.commit();
                let change = last_change(
                    call.services,
                    call.reading,
                    (&self.id, at),
                    LAST_CHANGE_DEADLINE,
                );
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
        let (services, reading) = (self.call.services, self.call.reading);
        if let Some(at) = self.call.tree.commit() {
            let history = rev::history_of(services, reading, &request, at);
            return Noted::new(refusal_text(history));
        }
        history(services, reading, &request).map(refusal_text)
    }

    fn body(&self) -> Noted<Result<Result<SymbolCard, String>, Error>> {
        let (services, reading) = (self.call.services, self.call.reading);
        if let Some(at) = self.call.tree.commit() {
            return Noted::new(Ok(rev::body_of(services, reading, at, &self.id)));
        }
        let request = ExpandRequest {
            ids: vec![self.id.clone()],
            limit: IntArg::from_usize(1),
        };
        let expanded = expand(self.call.services, self.call.reading, &request);
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
pub(super) fn refusal_text<T>(outcome: Result<T, Error>) -> Result<Result<T, String>, Error> {
    match outcome {
        Ok(value) => Ok(Ok(value)),
        Err(error @ Error::Kind(kernel::Error::CallStopped(_))) => Err(error),
        Err(error) => Ok(Err(error.to_string())),
    }
}

// The position of a tier in the precedence rule, strongest first.
fn tier_at(tier: &str) -> usize {
    (Tier::ORDER.iter())
        .position(|known| known.as_str() == tier)
        .unwrap_or(Tier::ORDER.len())
}

// Where a row stands in a section in full: the binding tiers together first, in the default
// card's order, then the other tiers in tier order. The sorts that use it are stable.
fn listing_rank(tier: &str) -> usize {
    if binds(tier) { 0 } else { tier_at(tier) }
}

fn callers(found: &RefsResult, more: Option<Section>) -> Callers {
    let listing = &found.groups;
    let whole = listing.omitted() == 0;
    let listed: Vec<&RefGroup> = if more == Some(Section::Callers) {
        let mut every: Vec<&RefGroup> = listing.rows.iter().collect();
        every.sort_by_key(|group| listing_rank(group.tier));
        every
    } else {
        (listing.rows.iter())
            .filter(|group| binds(group.tier) && !is_test_path(&group.path))
            .collect()
    };
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
        rows: (listed.iter())
            .take(cap(more, Section::Callers, SYMBOL_ROWS))
            .map(|group| (*group).clone())
            .collect(),
        rows_total: Tally::of(listed.len(), whole),
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

// Each candidate carries the strongest tier of its shared callers over the whole fan-in, so the
// rows and the counts agree when the listing of sites was cut.
fn similar(found: &RefsResult) -> Similar {
    let shared = &found.shared;
    let mut listed: Vec<&SharedCaller> = shared.rows().iter().collect();
    listed.sort_by_key(|candidate| listing_rank(candidate.tier));
    let rows = listed.into_iter().map(|candidate| SimilarRow {
        candidate: candidate.clone(),
        tier: candidate.tier,
    });
    Similar {
        total: shared.total(),
        rows: rows.collect(),
        bound_total: Tally::Exact(found.shared_bound),
    }
}

// Fan-out lists its tiers strongest first, so a section in full keeps that order.
fn callees(explained: &Explanation, more: Option<Section>) -> Callees {
    let full = more == Some(Section::Callees);
    let tiers = (explained.fan_out.iter())
        .map(|group| TierTotal {
            tier: group.tier,
            total: group.total,
        })
        .collect();
    let bound: usize = (explained.fan_out.iter())
        .filter(|group| binds(group.tier))
        .map(|group| group.total)
        .sum();
    let all: usize = explained.fan_out.iter().map(|group| group.total).sum();
    let listed: Vec<&TierGroup> = (explained.fan_out.iter())
        .filter(|group| full || binds(group.tier))
        .collect();
    let whole = listed.iter().all(|group| group.rows.len() == group.total);
    let groups: Vec<EdgeGroup> = listed.into_iter().flat_map(by_file).collect();
    Callees {
        bound,
        name_only: all - bound,
        rows_total: Tally::of(groups.len(), whole),
        rows: (groups.into_iter())
            .take(cap(more, Section::Callees, SYMBOL_ROWS))
            .collect(),
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
