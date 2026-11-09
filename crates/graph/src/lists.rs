//! The merge of a sorted list with the items that an update takes out of it and puts in it.

use std::cmp::Ordering;

// Takes the items at `removed` out of `held` and puts each of `added`, sorted, in its place; an
// added item equal to a kept one is left out, as a dedup leaves it out.
pub(crate) fn merged<T>(
    held: &mut Vec<T>,
    removed: &[usize],
    added: Vec<T>,
    order: impl Fn(&T, &T) -> Ordering,
) -> (Vec<usize>, Vec<usize>) {
    let gone = |at: usize| removed.binary_search(&at).is_ok();
    let points: Vec<Option<usize>> = (added.iter())
        .map(|item| {
            let point = held.partition_point(|held| order(held, item) == Ordering::Less);
            let equal = (held.get(point))
                .is_some_and(|held| order(held, item) == Ordering::Equal && !gone(point));
            (!equal).then_some(point)
        })
        .collect();
    let mut fresh = (added.into_iter().zip(points))
        .filter_map(|(item, point)| Some((item, point?)))
        .peekable();
    let first = [removed.first(), fresh.peek().map(|(_, point)| point)];
    let first = first
        .into_iter()
        .flatten()
        .min()
        .map_or(held.len(), |at| *at);
    let mut to: Vec<usize> = (0..held.len()).collect();
    let tail = held.split_off(first);
    let mut gone = removed.iter().copied().peekable();
    let mut placed = Vec::new();
    for (at, item) in (first..).zip(tail) {
        while let Some((item, _)) = fresh.next_if(|(_, point)| *point == at) {
            placed.push(held.len());
            held.push(item);
        }
        if gone.next_if_eq(&at).is_some() {
            to[at] = usize::MAX;
            continue;
        }
        to[at] = held.len();
        held.push(item);
    }
    for (item, _) in fresh {
        placed.push(held.len());
        held.push(item);
    }
    (to, placed)
}

// `held` with each of `added` at its point, a point being a position in `held`.
pub(crate) fn interleaved(held: Vec<usize>, added: Vec<usize>, points: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(held.len() + added.len());
    let mut fresh = added.into_iter().zip(points.iter().copied()).peekable();
    for (index, item) in held.into_iter().enumerate() {
        while let Some((at, _)) = fresh.next_if(|(_, point)| *point == index) {
            out.push(at);
        }
        out.push(item);
    }
    out.extend(fresh.map(|(at, _)| at));
    out
}
