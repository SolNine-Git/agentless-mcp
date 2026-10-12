//! The unit matching that `complexity_delta` and `contract_drift` share: body, name, then rank.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use metrics::Unit;

use crate::finding::Match;
use crate::pair::{Sides, metrics};

// `file` and `index` are the unit's identity within its side of the run.
#[derive(Clone, Copy)]
pub(crate) struct UnitRef<'a> {
    pub(crate) file: usize,
    pub(crate) index: usize,
    pub(crate) path: &'a str,
    pub(crate) unit: &'a Unit,
}

impl UnitRef<'_> {
    fn id(&self) -> (usize, usize) {
        (self.file, self.index)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Paired<'a> {
    pub(crate) old: Option<UnitRef<'a>>,
    pub(crate) new: UnitRef<'a>,
    pub(crate) via: Match,
}

pub(crate) struct FileUnits<'a> {
    pub(crate) old: Vec<UnitRef<'a>>,
    pub(crate) new: Vec<UnitRef<'a>>,
}

pub(crate) fn file_units<'a>(measured: &[Sides<'a>]) -> Vec<FileUnits<'a>> {
    measured
        .iter()
        .enumerate()
        .map(|(file, sides)| FileUnits {
            old: unit_refs(file, sides.pair.path(), &metrics(sides.old).units),
            new: unit_refs(file, sides.pair.path(), &metrics(sides.new).units),
        })
        .collect()
}

fn unit_refs<'a>(file: usize, path: &'a str, units: &'a [Unit]) -> Vec<UnitRef<'a>> {
    units
        .iter()
        .enumerate()
        .map(|(index, unit)| UnitRef {
            file,
            index,
            path,
            unit,
        })
        .collect()
}

// Each file pairs its own units first, so a copy beside its unchanged original is new in
// the other file, and a unit moved between files is not.
pub(crate) fn match_run<'a>(files: &[FileUnits<'a>]) -> Vec<Paired<'a>> {
    let mut matches = Vec::new();
    let mut old_left = Vec::new();
    let mut new_left = Vec::new();
    for file in files {
        let (paired, olds, news) = body_pass(&file.old, &file.new);
        matches.extend(paired);
        old_left.extend(olds);
        new_left.extend(news);
    }
    let (moved, old_rest, new_rest) = body_pass(&old_left, &new_left);
    matches.extend(moved);
    let old_by_file = lookup(grouped(&old_rest, |unit| unit.file));
    let new_by_file = lookup(grouped(&new_rest, |unit| unit.file));
    for file in 0..files.len() {
        let (named, unpaired) = name_and_collision_passes(
            old_by_file.get(&file).map_or(&[], Vec::as_slice),
            new_by_file.get(&file).map_or(&[], Vec::as_slice),
        );
        matches.extend(named);
        matches.extend(unpaired.into_iter().map(|new| Paired {
            old: None,
            new,
            via: Match::New,
        }));
    }
    matches
}

type Pass<'a> = (Vec<Paired<'a>>, Vec<UnitRef<'a>>, Vec<UnitRef<'a>>);

fn body_pass<'a>(old_units: &[UnitRef<'a>], new_units: &[UnitRef<'a>]) -> Pass<'a> {
    let old_by_body = lookup(grouped(old_units, |unit| unit.unit.body_key));
    let mut partner: HashMap<(usize, usize), UnitRef<'a>> = HashMap::new();
    for (body_key, news) in grouped(new_units, |unit| unit.unit.body_key) {
        let olds = old_by_body.get(&body_key).map_or(&[][..], Vec::as_slice);
        partner.extend(body_group(olds, &news));
    }
    let matches = new_units
        .iter()
        .filter_map(|new| {
            partner.get(&new.id()).map(|old| Paired {
                old: Some(*old),
                new: *new,
                via: Match::Body,
            })
        })
        .collect();
    let new_left = new_units
        .iter()
        .filter(|new| !partner.contains_key(&new.id()))
        .copied()
        .collect();
    let taken: HashSet<(usize, usize)> = partner.values().map(UnitRef::id).collect();
    let old_left = without(old_units, &taken);
    (matches, old_left, new_left)
}

// body_key omits the unit's own name, so a renamed copy has its original's key.
// Same names pair first, so a copy cannot take its original's old unit.
fn body_group<'a>(
    olds: &[UnitRef<'a>],
    news: &[UnitRef<'a>],
) -> Vec<((usize, usize), UnitRef<'a>)> {
    let old_named = lookup(grouped(olds, |unit| unit.unit.qualname.as_str()));
    let mut partner = Vec::new();
    for (qualname, named) in grouped(news, |unit| unit.unit.qualname.as_str()) {
        let same = old_named.get(qualname).map_or(&[][..], Vec::as_slice);
        partner.extend(named.iter().zip(same).map(|(new, old)| (new.id(), *old)));
    }
    let taken: HashSet<(usize, usize)> = partner.iter().map(|(_, old)| old.id()).collect();
    let paired: HashSet<(usize, usize)> = partner.iter().map(|(new, _)| *new).collect();
    let old_rest = without(olds, &taken);
    let new_rest = without(news, &paired);
    partner.extend(
        new_rest
            .iter()
            .zip(old_rest)
            .map(|(new, old)| (new.id(), old)),
    );
    partner
}

fn name_and_collision_passes<'a>(
    old_left: &[UnitRef<'a>],
    new_left: &[UnitRef<'a>],
) -> (Vec<Paired<'a>>, Vec<UnitRef<'a>>) {
    let old_named = lookup(grouped(old_left, |unit| unit.unit.qualname.as_str()));
    let mut matches = Vec::new();
    let mut unpaired = Vec::new();
    for (qualname, news) in grouped(new_left, |unit| unit.unit.qualname.as_str()) {
        let olds = old_named.get(qualname).map_or(&[][..], Vec::as_slice);
        if let ([old], [new]) = (olds, news.as_slice()) {
            matches.push(Paired {
                old: Some(*old),
                new: *new,
                via: Match::Name,
            });
            continue;
        }
        // A multiset by rank: ordinals shift when a same-name unit is inserted
        // above another, and a ranking does not.
        let ranked_old = ranked(olds);
        let ranked_new = ranked(&news);
        matches.extend(ranked_old.iter().zip(&ranked_new).map(|(old, new)| Paired {
            old: Some(*old),
            new: *new,
            via: Match::Collision,
        }));
        unpaired.extend(ranked_new.into_iter().skip(ranked_old.len()));
    }
    (matches, unpaired)
}

fn ranked<'a>(units: &[UnitRef<'a>]) -> Vec<UnitRef<'a>> {
    let mut ranked = units.to_vec();
    ranked.sort_by_key(|unit| (Reverse(unit.unit.cc), unit.unit.start_byte));
    ranked
}

fn without<'a>(units: &[UnitRef<'a>], gone: &HashSet<(usize, usize)>) -> Vec<UnitRef<'a>> {
    units
        .iter()
        .filter(|unit| !gone.contains(&unit.id()))
        .copied()
        .collect()
}

// Groups in the order each key first appears, each group in input order.
fn grouped<'a, K: Hash + Eq + Copy>(
    units: &[UnitRef<'a>],
    key: impl Fn(&UnitRef<'a>) -> K,
) -> Vec<(K, Vec<UnitRef<'a>>)> {
    let mut slots: HashMap<K, usize> = HashMap::new();
    let mut groups: Vec<(K, Vec<UnitRef<'a>>)> = Vec::new();
    for unit in units {
        let group = key(unit);
        let slot = *slots.entry(group).or_insert_with(|| {
            groups.push((group, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(*unit);
    }
    groups
}

fn lookup<K: Hash + Eq>(groups: Vec<(K, Vec<UnitRef<'_>>)>) -> HashMap<K, Vec<UnitRef<'_>>> {
    groups.into_iter().collect()
}
