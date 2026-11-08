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
    let old = std::mem::take(held);
    let mut to = vec![usize::MAX; old.len()];
    let mut gone = removed.iter().copied().peekable();
    let mut kept: Vec<T> = Vec::with_capacity(old.len());
    let mut kept_from: Vec<usize> = Vec::with_capacity(old.len());
    for (at, item) in old.into_iter().enumerate() {
        if gone.next_if_eq(&at).is_some() {
            continue;
        }
        kept.push(item);
        kept_from.push(at);
    }
    let points: Vec<Option<usize>> = (added.iter())
        .map(|item| {
            let point = kept.partition_point(|held| order(held, item) == Ordering::Less);
            let equal = kept
                .get(point)
                .is_some_and(|held| order(held, item) == Ordering::Equal);
            (!equal).then_some(point)
        })
        .collect();
    let mut fresh = added
        .into_iter()
        .zip(points)
        .filter_map(|(item, point)| Some((item, point?)));
    let mut placed = Vec::new();
    let mut out: Vec<T> = Vec::with_capacity(kept.len());
    let mut next = fresh.next();
    for (index, (item, from)) in kept.into_iter().zip(kept_from).enumerate() {
        while let Some((_, point)) = &next
            && *point == index
        {
            let (item, _) = next.take().expect("the loop holds an item");
            placed.push(out.len());
            out.push(item);
            next = fresh.next();
        }
        to[from] = out.len();
        out.push(item);
    }
    while let Some((item, _)) = next {
        placed.push(out.len());
        out.push(item);
        next = fresh.next();
    }
    *held = out;
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
