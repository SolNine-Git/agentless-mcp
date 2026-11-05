//! `callee_overlap`: each function a change adds or rewrites that calls most of the functions an
//! existing function calls, but does not call that function.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap};

use metrics::Unit;
use prompts::messages;
use syntax::Parsed;

use crate::error::Error;
use crate::finding::{
    CalleeShare, Detail, Finding, Match, PairFinding, Severity, SharedCallee, TieredCallee,
    not_checked, sorted,
};
use crate::matching::{Paired, file_units, match_run};
use crate::observed::{Observed, Outcome};
use crate::pair::{CALLEE_GAPS, Pair, Sides, each_pair, read_sides};

// An existing function needs this many counted callees before a copy of it can be found.
const MIN_CALLEES: usize = 3;
// The share is `SHARE_PARTS / SHARE_WHOLE` (80%) of the existing function's callees.
const SHARE_PARTS: usize = 4;
const SHARE_WHOLE: usize = 5;
// This many other functions that meet the rule for one function are a convention, not a copy.
const FAMILY_LIMIT: usize = 5;
const MAX_ROWS_PER_UNIT: usize = 3;

/// The callee sets of every function the check reads in the tree after the change, or the stop
/// of their build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalleeTree {
    /// Every function in the check's scope.
    Built(Vec<TreeFunction>),
    /// The scan or the graph build reached the time budget of `seconds`.
    Stopped {
        /// The time budget, in whole seconds.
        seconds: u64,
    },
}

/// One function of the tree after the change, with the functions it calls at a binding tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFunction {
    /// The repo-relative path.
    pub path: String,
    /// The function's own name.
    pub name: String,
    /// The qualified name.
    pub qualname: String,
    /// The first line, at the first attribute or decorator.
    pub line: u32,
    /// The line of the name, which is the first line of the function's metrics unit.
    pub name_line: u32,
    /// Each counted callee once, in id order.
    pub callees: Vec<Callee>,
    /// Each name that the function's lines spell, sorted, each once.
    pub spelled: Vec<String>,
}

/// One function that an edge of a tree function reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callee {
    /// The callee's stable id.
    pub id: String,
    /// The callee's own name.
    pub name: String,
    /// The callee's qualified name.
    pub qualname: String,
    /// The evidence tier of the edge, as its wire value.
    pub tier: &'static str,
}

/// Report each new or rewritten function that calls at least 80% of the three or more counted
/// callees of an existing function of `tree`, and does not spell that function's name.
///
/// # Errors
///
/// Returns an error when two pairs name one path, or when a side fails in a way that is not a
/// parse or tokenizer failure.
pub fn callee_overlap(pairs: &[&Pair], tree: &CalleeTree) -> Result<Outcome, Error> {
    let functions = match tree {
        CalleeTree::Stopped { seconds } => {
            let budget = usize::try_from(*seconds).unwrap_or(usize::MAX);
            let message = messages::TREE_CHECK_STOPPED.render((
                crate::CALLEE_OVERLAP,
                messages::CALLEE_OVERLAP_READS,
                budget,
            ));
            let evidence = messages::TREE_CHECK_STOPPED_EVIDENCE;
            let row = not_checked(crate::CALLEE_OVERLAP, "", message, evidence, None);
            return Ok(Outcome {
                findings: vec![row],
                observed: Observed::default(),
            });
        }
        CalleeTree::Built(functions) => functions,
    };
    let (measured, mut findings) = each_pair(pairs, |pair, language| {
        read_sides(pair, language, &CALLEE_GAPS)
    })?;
    let matches = match_run(&file_units(&measured));
    let index = Index::new(functions);
    let checked: Vec<Checked<'_>> = matches
        .iter()
        .filter_map(|paired| index.checked(paired, &measured))
        .collect();
    let mut families = Families::default();
    let rows: Vec<Row<'_>> = checked
        .iter()
        .flat_map(|unit| index.rows(unit, &mut families))
        .collect();
    let kept = once_per_pair(&index, rows);
    findings.extend(kept.iter().map(|row| finding(&index, row)));
    Ok(Outcome {
        findings: sorted(findings),
        observed: Observed::default(),
    })
}

// A unit the check reads: new, or rewritten with the names its old body spelled.
struct Checked<'a> {
    paired: &'a Paired<'a>,
    function: usize,
    old_spelled: Option<BTreeSet<&'a str>>,
}

// One candidate row: the unit's function f and the existing function g it overlaps.
struct Row<'a> {
    unit: &'a Checked<'a>,
    other: usize,
    shared: Vec<(usize, usize)>,
    family: usize,
}

type Rank<'f> = (Reverse<usize>, Reverse<usize>, &'f str, u32, usize);

struct Index<'f> {
    functions: &'f [TreeFunction],
    by_place: HashMap<(&'f str, &'f str, u32), usize>,
    callers: HashMap<&'f str, Vec<(usize, usize)>>,
}

impl<'f> Index<'f> {
    fn new(functions: &'f [TreeFunction]) -> Index<'f> {
        let mut by_place = HashMap::new();
        let mut callers: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
        for (at, function) in functions.iter().enumerate() {
            let place = (
                function.path.as_str(),
                function.name.as_str(),
                function.name_line,
            );
            by_place.entry(place).or_insert(at);
            for (slot, callee) in function.callees.iter().enumerate() {
                callers
                    .entry(callee.id.as_str())
                    .or_default()
                    .push((at, slot));
            }
        }
        Index {
            functions,
            by_place,
            callers,
        }
    }

    // A new unit, or a rewritten one with its old names, joined to its tree function.
    fn checked<'a>(&self, paired: &'a Paired<'a>, measured: &'a [Sides<'a>]) -> Option<Checked<'a>>
    where
        'f: 'a,
    {
        let old_spelled = match (paired.via, paired.old) {
            (Match::New, _) => None,
            (Match::Name | Match::Collision, Some(old))
                if old.unit.body_key != paired.new.unit.body_key =>
            {
                let parsed = measured[old.file].old?.parsed();
                Some(spelled_in(parsed, old.unit))
            }
            _ => return None,
        };
        let new = paired.new.unit;
        let name = own_name(&new.qualname);
        let function = *self
            .by_place
            .get(&(paired.new.path, name, new.start_line))?;
        Some(Checked {
            paired,
            function,
            old_spelled,
        })
    }

    // The rows of one unit: the existing functions it meets the rule for, best first.
    fn rows<'a>(&self, unit: &'a Checked<'a>, families: &mut Families) -> Vec<Row<'a>> {
        let f = unit.function;
        let mut shared: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for (slot, callee) in self.functions[f].callees.iter().enumerate() {
            for &(g, other_slot) in self.callers.get(callee.id.as_str()).into_iter().flatten() {
                shared.entry(g).or_default().push((slot, other_slot));
            }
        }
        let mut rows: Vec<Row<'a>> = shared
            .into_iter()
            .filter(|(g, pairs)| *g != f && self.meets(f, *g, pairs.len()))
            .filter(|(g, _)| {
                !unit
                    .old_spelled
                    .as_ref()
                    .is_some_and(|old| self.old_met(old, *g))
            })
            .filter_map(|(g, pairs)| {
                let family = families.of(self, g, f);
                (family < FAMILY_LIMIT).then_some(Row {
                    unit,
                    other: g,
                    shared: pairs,
                    family,
                })
            })
            .collect();
        rows.sort_by(|a, b| self.rank(a).cmp(&self.rank(b)));
        rows.truncate(MAX_ROWS_PER_UNIT);
        rows
    }

    // The rule for f against g: g has enough callees, f calls 80% of them, f never spells g.
    fn meets(&self, f: usize, g: usize, shared: usize) -> bool {
        let other = &self.functions[g];
        let total = other.callees.len();
        total >= MIN_CALLEES
            && SHARE_WHOLE * shared >= SHARE_PARTS * total
            && !spells(&self.functions[f], &other.name)
    }

    // The old body already spelled 80% of g's callee names, so the rewrite did not make the copy.
    fn old_met(&self, old: &BTreeSet<&str>, g: usize) -> bool {
        let callees = &self.functions[g].callees;
        let spelled = callees
            .iter()
            .filter(|callee| old.contains(callee.name.as_str()))
            .count();
        SHARE_WHOLE * spelled >= SHARE_PARTS * callees.len()
    }

    // More shared callees first, then the larger set of g, then g's path and line, then the tree
    // order, which breaks a tie of two functions on one line.
    fn rank(&self, row: &Row<'_>) -> Rank<'f> {
        let other = &self.functions[row.other];
        (
            Reverse(row.shared.len()),
            Reverse(other.callees.len()),
            other.path.as_str(),
            other.line,
            row.other,
        )
    }

    // Every function besides g that meets the rule for g.
    fn members(&self, g: usize) -> Vec<usize> {
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for callee in &self.functions[g].callees {
            for &(h, _) in self.callers.get(callee.id.as_str()).into_iter().flatten() {
                *counts.entry(h).or_default() += 1;
            }
        }
        let mut members: Vec<usize> = counts
            .into_iter()
            .filter(|(h, shared)| *h != g && self.meets(*h, g, *shared))
            .map(|(h, _)| h)
            .collect();
        members.sort_unstable();
        members
    }
}

// The members of each g's family, found once per run.
#[derive(Default)]
struct Families {
    members: HashMap<usize, Vec<usize>>,
}

impl Families {
    // How many functions besides f and g meet the rule for g.
    fn of(&mut self, index: &Index<'_>, g: usize, f: usize) -> usize {
        let members = self.members.entry(g).or_insert_with(|| index.members(g));
        members.len() - usize::from(members.binary_search(&f).is_ok())
    }
}

// A pair of two checked units that meet the rule both ways keeps the row of the unit that comes
// first by path and line. A checked unit's function starts its name on the unit's first line.
fn once_per_pair<'a>(index: &Index<'_>, rows: Vec<Row<'a>>) -> Vec<Row<'a>> {
    let found: BTreeSet<(usize, usize)> = rows
        .iter()
        .map(|row| (row.unit.function, row.other))
        .collect();
    let place = |at: usize| {
        let function = &index.functions[at];
        (function.path.as_str(), function.name_line)
    };
    rows.into_iter()
        .filter(|row| {
            let (f, g) = (row.unit.function, row.other);
            !found.contains(&(g, f)) || place(f) < place(g)
        })
        .collect()
}

fn finding(index: &Index<'_>, row: &Row<'_>) -> PairFinding {
    let paired = row.unit.paired;
    let f = &index.functions[row.unit.function];
    let g = &index.functions[row.other];
    let unit = paired.new.unit;
    let mut shared: Vec<SharedCallee> = row
        .shared
        .iter()
        .map(|&(slot, other_slot)| SharedCallee {
            callee: f.callees[slot].id.clone(),
            tier: f.callees[slot].tier,
            other_tier: g.callees[other_slot].tier,
        })
        .collect();
    shared.sort_by(|a, b| a.callee.cmp(&b.callee));
    let mut labels: Vec<&str> = row
        .shared
        .iter()
        .map(|&(slot, _)| f.callees[slot].qualname.as_str())
        .collect();
    labels.sort_unstable();
    let message = messages::CALLEE_OVERLAP_FOUND.render((
        g.qualname.as_str(),
        unit.qualname.as_str(),
        row.shared.len(),
        g.callees.len(),
    ));
    let evidence = messages::CALLEE_OVERLAP_FOUND_EVIDENCE.render((
        g.path.as_str(),
        g.qualname.as_str(),
        g.line as usize,
        labels.join(", ").as_str(),
    ));
    let detail = CalleeShare {
        unit: unit.qualname.clone(),
        via: paired.via,
        other: g.qualname.clone(),
        other_path: g.path.clone(),
        other_line: g.line,
        family: row.family,
        shared,
        other_callees: g
            .callees
            .iter()
            .map(|callee| TieredCallee {
                callee: callee.id.clone(),
                tier: callee.tier,
            })
            .collect(),
    };
    PairFinding {
        finding: Finding {
            check: crate::CALLEE_OVERLAP,
            severity: Severity::Advisory,
            message,
            path: paired.new.path.to_owned(),
            line: unit.start_line,
            evidence,
        },
        detail: Some(Detail::Overlap(Box::new(detail))),
    }
}

fn spells(function: &TreeFunction, name: &str) -> bool {
    function
        .spelled
        .binary_search_by(|spelled| spelled.as_str().cmp(name))
        .is_ok()
}

// The identifiers that a parse spells inside `unit`.
fn spelled_in<'t>(parsed: &'t Parsed, unit: &Unit) -> BTreeSet<&'t str> {
    let identifiers = &parsed.language().extraction().identifiers;
    parsed
        .descendants(parsed.root())
        .filter(|node| identifiers.contains(parsed.kind(*node)))
        .filter(|node| {
            unit.start_byte <= parsed.start_byte(*node) && parsed.end_byte(*node) <= unit.end_byte
        })
        .map(|node| parsed.text(node))
        .collect()
}

// A metrics qualname joins its scopes with dots, so the own name is its last part.
fn own_name(qualname: &str) -> &str {
    qualname.rsplit_once('.').map_or(qualname, |(_, name)| name)
}

#[cfg(test)]
mod tests;
