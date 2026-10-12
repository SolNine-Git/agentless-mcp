//! The blocks of one `clone_delta` run, the blocks a change added, and their instances.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

use kernel::Key;
use metrics::Block;

use crate::diff::matching_blocks;
use crate::index::IndexedFiles;
use crate::pair::{Measured, Sides, blocks};

pub(crate) const MIN_FINDING_TOKENS: u32 = 40;

#[derive(Clone, Copy)]
pub(crate) struct Placed<'a> {
    pub(crate) path: &'a str,
    pub(crate) block: &'a Block,
}

impl Placed<'_> {
    pub(crate) fn place(&self) -> (&str, u32, u32) {
        metrics::place(self.path, self.block)
    }
}

struct Indexed {
    old: Vec<usize>,
    new: Vec<usize>,
    unchanged: HashMap<usize, usize>,
}

// Each block of the run once, by id; the instances of a Type-2 key are the repository after
// the change: every index file the run does not change, then each pair's new text.
pub(crate) struct Run<'a> {
    blocks: Vec<Placed<'a>>,
    indexed: Vec<Indexed>,
    instances: HashMap<Key, Vec<usize>>,
}

impl<'a> Run<'a> {
    pub(crate) fn new(
        files: &'a IndexedFiles,
        read: &[Sides<'a>],
        changed: &HashSet<&str>,
    ) -> Self {
        let mut run = Run {
            blocks: Vec::new(),
            indexed: Vec::new(),
            instances: HashMap::new(),
        };
        for (path, file) in &files.files {
            if !changed.contains(path.as_str()) {
                let ids = run.push_all(path, file);
                run.add_instances(&ids);
            }
        }
        for sides in read {
            let path = sides.pair.path();
            let new = run.push_all(path, blocks(sides.new));
            run.add_instances(&new);
            let old = run.push_all(path, blocks(sides.old));
            let unchanged = unchanged_lines(sides.old, sides.new);
            run.indexed.push(Indexed {
                old,
                new,
                unchanged,
            });
        }
        run
    }

    fn push_all(&mut self, path: &'a str, blocks: &'a [Block]) -> Vec<usize> {
        let first = self.blocks.len();
        self.blocks
            .extend(blocks.iter().map(|block| Placed { path, block }));
        (first..self.blocks.len()).collect()
    }

    fn add_instances(&mut self, ids: &[usize]) {
        for &id in ids {
            let key = self.blocks[id].block.key;
            self.instances.entry(key).or_default().push(id);
        }
    }

    pub(crate) fn placed(&self, id: usize) -> Placed<'a> {
        self.blocks[id]
    }

    pub(crate) fn instances(&self, id: usize) -> &[usize] {
        self.instances
            .get(&self.blocks[id].block.key)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn new_blocks(&self) -> impl Iterator<Item = usize> + '_ {
        self.indexed
            .iter()
            .flat_map(|item| item.new.iter().copied())
    }

    pub(crate) fn reportable(&self, id: usize) -> bool {
        self.blocks[id].block.tokens >= MIN_FINDING_TOKENS
            && self.instances(id).len() >= metrics::MIN_INSTANCES
    }

    pub(crate) fn sort_by_place(&self, ids: &mut [usize]) {
        ids.sort_by(|a, b| self.blocks[*a].place().cmp(&self.blocks[*b].place()));
    }

    // Each file pairs its own blocks first, so a copy beside its original is the added one,
    // and only then do the leftovers pair across files as moves.
    pub(crate) fn added(&self) -> Vec<usize> {
        let mut old_left = Vec::new();
        let mut new_left = Vec::new();
        for item in &self.indexed {
            let (olds, news) = self.within_file(item);
            old_left.extend(olds);
            new_left.extend(news);
        }
        self.sort_by_place(&mut old_left);
        self.sort_by_place(&mut new_left);
        let (olds, news) = self.pair_by(
            &old_left,
            &new_left,
            |id| self.exact(id),
            |id| self.exact(id),
        );
        let (_, news) = self.pair_by(&olds, &news, |id| self.type2(id), |id| self.type2(id));
        news
    }

    // A block on lines the diff left in place pairs with itself before any copy of it can.
    fn within_file(&self, item: &Indexed) -> (Vec<usize>, Vec<usize>) {
        let line = |id: usize| self.blocks[id].block.start_line as usize;
        let (olds, news) = self.pair_by(
            &item.old,
            &item.new,
            |id| (item.unchanged.get(&line(id)).copied(), self.exact(id)),
            |id| (Some(line(id)), self.exact(id)),
        );
        let (olds, news) = self.pair_by(&olds, &news, |id| self.exact(id), |id| self.exact(id));
        self.pair_by(&olds, &news, |id| self.type2(id), |id| self.type2(id))
    }

    fn exact(&self, id: usize) -> Key {
        self.blocks[id].block.exact
    }

    fn type2(&self, id: usize) -> Key {
        self.blocks[id].block.key
    }

    // Old blocks wait in a queue per key, in the order each key first appears; each new block
    // takes the first one waiting under its key. The old blocks left over come back by place.
    fn pair_by<K: Hash + Eq>(
        &self,
        old: &[usize],
        new: &[usize],
        old_key: impl Fn(usize) -> K,
        new_key: impl Fn(usize) -> K,
    ) -> (Vec<usize>, Vec<usize>) {
        let mut slots: HashMap<K, usize> = HashMap::new();
        let mut waiting: Vec<VecDeque<usize>> = Vec::new();
        for &id in old {
            let slot = *slots.entry(old_key(id)).or_insert_with(|| {
                waiting.push(VecDeque::new());
                waiting.len() - 1
            });
            waiting[slot].push_back(id);
        }
        let new_left = new
            .iter()
            .copied()
            .filter(|id| {
                let slot = slots.get(&new_key(*id));
                slot.and_then(|slot| waiting[*slot].pop_front()).is_none()
            })
            .collect();
        let mut old_left: Vec<usize> = waiting.into_iter().flatten().collect();
        self.sort_by_place(&mut old_left);
        (old_left, new_left)
    }

    // A copy inside a reported copy folds into it, so one pasted function is one finding.
    pub(crate) fn outermost(&self, mut copies: Vec<usize>) -> Vec<usize> {
        copies.sort_by_key(|id| {
            let placed = self.blocks[*id];
            (
                placed.path,
                placed.block.start_line,
                Reverse(placed.block.end_line),
            )
        });
        let mut kept = Vec::new();
        let mut outer: Option<usize> = None;
        for id in copies {
            if outer.is_some_and(|outer| self.within(id, outer)) {
                continue;
            }
            outer = Some(id);
            kept.push(id);
        }
        kept
    }

    fn within(&self, id: usize, outer: usize) -> bool {
        let (inner, outer) = (self.blocks[id], self.blocks[outer]);
        inner.path == outer.path
            && outer.block.start_line <= inner.block.start_line
            && inner.block.end_line <= outer.block.end_line
    }
}

// Rows split on "\n" alone, as tree-sitter numbers them; the map takes 1-based old lines to
// the new lines the diff kept in place.
fn unchanged_lines(old: Option<&Measured>, new: Option<&Measured>) -> HashMap<usize, usize> {
    let (Some(old), Some(new)) = (old, new) else {
        return HashMap::new();
    };
    let old_rows: Vec<&str> = old.parsed().source().split('\n').collect();
    let new_rows: Vec<&str> = new.parsed().source().split('\n').collect();
    matching_blocks(&old_rows, &new_rows)
        .into_iter()
        .flat_map(|(first, second, size)| {
            (0..size).map(move |at| (first + at + 1, second + at + 1))
        })
        .collect()
}
