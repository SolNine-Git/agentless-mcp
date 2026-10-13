//! The import cycles the resolved imports prove: strongly connected components of two or more
//! files, each with one shortest chain that closes it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::symbols::SymbolGraph;

// A component of one file is not a cycle.
const SMALLEST_CYCLE: usize = 2;

/// One import cycle: a chain of files that closes on its first file, and every file of the
/// strongly connected component it runs through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cycle<'a> {
    pub files: Vec<&'a str>,
    pub members: Vec<&'a str>,
}

pub(crate) type Adjacency<'a> = BTreeMap<&'a str, Vec<&'a str>>;

/// Return the import cycles of `graph`, ordered by chain length, then by chain.
#[must_use]
pub fn import_cycles<'a>(graph: &SymbolGraph<'a>) -> Vec<Cycle<'a>> {
    let mut adjacency: Adjacency<'a> = BTreeMap::new();
    for edge in graph.import_edges() {
        adjacency
            .entry(edge.source().path())
            .or_default()
            .push(edge.target().path());
    }
    for targets in adjacency.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }
    let mut cycles: Vec<Cycle<'a>> = components(graph.files(), &adjacency)
        .into_iter()
        .filter(|members| members.len() >= SMALLEST_CYCLE)
        .filter_map(|members| {
            let files = cycle_chain(&members, &adjacency)?;
            Some(Cycle { files, members })
        })
        .collect();
    cycles.sort_by(|left, right| {
        (left.files.len(), &left.files).cmp(&(right.files.len(), &right.files))
    });
    cycles
}

// Tarjan's components, iteratively: a long import chain must not exhaust the stack.
pub(crate) fn components<'a>(nodes: &[&'a str], adjacency: &Adjacency<'a>) -> Vec<Vec<&'a str>> {
    let mut tarjan = Tarjan {
        adjacency,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        found: Vec::new(),
        counter: 0,
    };
    for &start in nodes {
        if !tarjan.index.contains_key(start) {
            tarjan.walk(start);
        }
    }
    tarjan.found
}

struct Tarjan<'g, 'a> {
    adjacency: &'g Adjacency<'a>,
    index: BTreeMap<&'a str, usize>,
    low: BTreeMap<&'a str, usize>,
    on_stack: BTreeSet<&'a str>,
    stack: Vec<&'a str>,
    found: Vec<Vec<&'a str>>,
    counter: usize,
}

impl<'a> Tarjan<'_, 'a> {
    fn walk(&mut self, start: &'a str) {
        let mut work: Vec<(&'a str, usize)> = vec![(start, 0)];
        while let Some((node, position)) = work.pop() {
            if position == 0 {
                self.open(node);
            }
            let neighbours = self.adjacency.get(node).map_or(&[][..], Vec::as_slice);
            if let Some(&child) = neighbours.get(position) {
                work.push((node, position + 1));
                self.advance(node, child, &mut work);
                continue;
            }
            self.close(node);
            if let Some(&(parent, _)) = work.last() {
                self.lower(parent, self.low[node]);
            }
        }
    }

    fn open(&mut self, node: &'a str) {
        self.index.insert(node, self.counter);
        self.low.insert(node, self.counter);
        self.counter += 1;
        self.stack.push(node);
        self.on_stack.insert(node);
    }

    fn advance(&mut self, node: &'a str, child: &'a str, work: &mut Vec<(&'a str, usize)>) {
        if !self.index.contains_key(child) {
            work.push((child, 0));
        } else if self.on_stack.contains(child) {
            self.lower(node, self.index[child]);
        }
    }

    fn lower(&mut self, node: &'a str, value: usize) {
        if let Some(low) = self.low.get_mut(node) {
            *low = (*low).min(value);
        }
    }

    // A node whose low link is its own index roots a component: pop it off the stack.
    fn close(&mut self, node: &'a str) {
        if self.low[node] != self.index[node] {
            return;
        }
        let mut component = Vec::new();
        while let Some(member) = self.stack.pop() {
            self.on_stack.remove(member);
            component.push(member);
            if member == node {
                break;
            }
        }
        component.sort_unstable();
        self.found.push(component);
    }
}

// Breadth first from the first member over sorted edges inside the component: the shortest
// cycle through that member.
fn cycle_chain<'a>(members: &[&'a str], adjacency: &Adjacency<'a>) -> Option<Vec<&'a str>> {
    let inside: BTreeSet<&str> = members.iter().copied().collect();
    let start = *members.first()?;
    let mut previous: BTreeMap<&'a str, &'a str> = BTreeMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for &child in adjacency.get(node).map_or(&[][..], Vec::as_slice) {
            if child == start {
                return Some(chain_back(node, start, &previous));
            }
            if inside.contains(child) && !previous.contains_key(child) {
                previous.insert(child, node);
                queue.push_back(child);
            }
        }
    }
    None
}

fn chain_back<'a>(
    node: &'a str,
    start: &str,
    previous: &BTreeMap<&'a str, &'a str>,
) -> Vec<&'a str> {
    let mut chain = vec![node];
    while let Some(&last) = chain.last().filter(|last| **last != start) {
        chain.push(previous[last]);
    }
    chain.reverse();
    chain
}
