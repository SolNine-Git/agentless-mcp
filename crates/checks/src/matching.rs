//! The unit matching that the pair checks and the diff card share: body, name, then rank.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use kernel::Key;
use metrics::Unit;

use crate::finding::Match;
use crate::pair::{Sides, units};

/// What the matcher reads of a unit: the key of its body, its qualified name, and its rank among
/// the units of one name.
pub trait Matchable {
    /// The key of a body with the unit's own name left out.
    type Body: Hash + Eq + Copy;
    /// The order among units of one name: the lower ranks first.
    type Rank: Ord;
    /// Returns the key of the unit's body.
    fn body_key(&self) -> Self::Body;
    /// Returns the unit's qualified name.
    fn qualname(&self) -> &str;
    /// Returns the unit's rank among the units of its name.
    fn rank(&self) -> Self::Rank;
}

impl Matchable for Unit {
    type Body = Key;
    type Rank = (Reverse<u32>, u32);

    fn body_key(&self) -> Key {
        self.body_key
    }

    fn qualname(&self) -> &str {
        &self.qualname
    }

    fn rank(&self) -> (Reverse<u32>, u32) {
        (Reverse(self.cc), self.start_byte)
    }
}

/// One pairing of [`pair_units`]: the file and index of the old unit, if one pairs, and of the new
/// unit, and how they paired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pairing {
    /// The file and the index in its old side of the old unit, or `None` for a new one.
    pub old: Option<(usize, usize)>,
    /// The file and the index in its new side of the new unit.
    pub new: (usize, usize),
    /// How the two paired.
    pub via: Match,
}

/// Pairs the old and the new units of each file, as the pair checks do: same body in the file,
/// then same body in another file, then one name, then rank. An old unit that no pairing names
/// was removed.
#[must_use]
pub fn pair_units<U: Matchable>(files: &[(&[U], &[U])]) -> Vec<Pairing> {
    let files: Vec<FileUnits<'_, U>> = (files.iter().enumerate())
        .map(|(file, (old, new))| FileUnits {
            old: unit_refs(file, "", old),
            new: unit_refs(file, "", new),
        })
        .collect();
    let paired = match_run(&files);
    (paired.iter())
        .map(|paired| Pairing {
            old: paired.old.map(|old| old.id()),
            new: paired.new.id(),
            via: paired.via,
        })
        .collect()
}

// `file` and `index` are the unit's identity within its side of the run.
pub(crate) struct UnitRef<'a, U = Unit> {
    pub(crate) file: usize,
    pub(crate) index: usize,
    pub(crate) path: &'a str,
    pub(crate) unit: &'a U,
}

// A derive would ask `U: Copy`; the reference is Copy for any unit.
impl<U> Clone for UnitRef<'_, U> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<U> Copy for UnitRef<'_, U> {}

impl<U> UnitRef<'_, U> {
    fn id(&self) -> (usize, usize) {
        (self.file, self.index)
    }
}

pub(crate) struct Paired<'a, U = Unit> {
    pub(crate) old: Option<UnitRef<'a, U>>,
    pub(crate) new: UnitRef<'a, U>,
    pub(crate) via: Match,
}

impl<U> Clone for Paired<'_, U> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<U> Copy for Paired<'_, U> {}

pub(crate) struct FileUnits<'a, U = Unit> {
    pub(crate) old: Vec<UnitRef<'a, U>>,
    pub(crate) new: Vec<UnitRef<'a, U>>,
}

pub(crate) fn file_units<'a>(measured: &[Sides<'a>]) -> Vec<FileUnits<'a>> {
    measured
        .iter()
        .enumerate()
        .map(|(file, sides)| FileUnits {
            old: unit_refs(file, sides.pair.path(), units(sides.old)),
            new: unit_refs(file, sides.pair.path(), units(sides.new)),
        })
        .collect()
}

fn unit_refs<'a, U>(file: usize, path: &'a str, units: &'a [U]) -> Vec<UnitRef<'a, U>> {
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
pub(crate) fn match_run<'a, U: Matchable>(files: &[FileUnits<'a, U>]) -> Vec<Paired<'a, U>> {
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

type Pass<'a, U> = (Vec<Paired<'a, U>>, Vec<UnitRef<'a, U>>, Vec<UnitRef<'a, U>>);

fn body_pass<'a, U: Matchable>(
    old_units: &[UnitRef<'a, U>],
    new_units: &[UnitRef<'a, U>],
) -> Pass<'a, U> {
    let old_by_body = lookup(grouped(old_units, |unit| unit.unit.body_key()));
    let mut partner: HashMap<(usize, usize), UnitRef<'a, U>> = HashMap::new();
    for (body_key, news) in grouped(new_units, |unit| unit.unit.body_key()) {
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
fn body_group<'a, U: Matchable>(
    olds: &[UnitRef<'a, U>],
    news: &[UnitRef<'a, U>],
) -> Vec<((usize, usize), UnitRef<'a, U>)> {
    let old_named = lookup(grouped(olds, |unit| unit.unit.qualname()));
    let mut partner = Vec::new();
    for (qualname, named) in grouped(news, |unit| unit.unit.qualname()) {
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

fn name_and_collision_passes<'a, U: Matchable>(
    old_left: &[UnitRef<'a, U>],
    new_left: &[UnitRef<'a, U>],
) -> (Vec<Paired<'a, U>>, Vec<UnitRef<'a, U>>) {
    let old_named = lookup(grouped(old_left, |unit| unit.unit.qualname()));
    let mut matches = Vec::new();
    let mut unpaired = Vec::new();
    for (qualname, news) in grouped(new_left, |unit| unit.unit.qualname()) {
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

fn ranked<'a, U: Matchable>(units: &[UnitRef<'a, U>]) -> Vec<UnitRef<'a, U>> {
    let mut ranked = units.to_vec();
    ranked.sort_by_key(|unit| unit.unit.rank());
    ranked
}

fn without<'a, U>(units: &[UnitRef<'a, U>], gone: &HashSet<(usize, usize)>) -> Vec<UnitRef<'a, U>> {
    units
        .iter()
        .filter(|unit| !gone.contains(&unit.id()))
        .copied()
        .collect()
}

// Groups in the order each key first appears, each group in input order.
fn grouped<'a, U, K: Hash + Eq + Copy>(
    units: &[UnitRef<'a, U>],
    key: impl Fn(&UnitRef<'a, U>) -> K,
) -> Vec<(K, Vec<UnitRef<'a, U>>)> {
    let mut slots: HashMap<K, usize> = HashMap::new();
    let mut groups: Vec<(K, Vec<UnitRef<'a, U>>)> = Vec::new();
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

fn lookup<K: Hash + Eq, U>(
    groups: Vec<(K, Vec<UnitRef<'_, U>>)>,
) -> HashMap<K, Vec<UnitRef<'_, U>>> {
    groups.into_iter().collect()
}
