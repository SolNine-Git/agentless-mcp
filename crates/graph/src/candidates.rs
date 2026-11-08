//! The definitions of one name in candidate order: by path, line and qualified name, each path's
//! definitions in one range.

use crate::names::Definition;

// The definitions of one name, ordered by (path, line, qualified name), and their path groups.
#[derive(Default)]
pub(crate) struct Candidates<'a> {
    pub(crate) ordered: Vec<Definition<'a>>,
    pub(crate) groups: Vec<(&'a str, usize, usize)>,
}

impl<'a> Candidates<'a> {
    // The sort is stable, so definitions with one key keep the order of `definitions`.
    pub(crate) fn of(definitions: &[Definition<'a>]) -> Candidates<'a> {
        let ordered = sorted(definitions);
        let groups = groups_of(&ordered);
        Candidates { ordered, groups }
    }

    pub(crate) fn group(&self, path: &str) -> &[Definition<'a>] {
        self.groups
            .binary_search_by(|(held, _, _)| (*held).cmp(path))
            .map_or(&[], |found| {
                let (_, start, end) = self.groups[found];
                &self.ordered[start..end]
            })
    }

    pub(crate) fn groups(&self) -> impl Iterator<Item = (&'a str, &[Definition<'a>])> {
        (self.groups.iter()).map(|&(path, start, end)| (path, &self.ordered[start..end]))
    }

    // Puts `definitions`, all of the file at `path` and in its symbol order, in place of the
    // path's group, as an ordering of every definition of the name puts them.
    pub(crate) fn replace(&mut self, path: &str, definitions: &[Definition<'a>]) {
        let found = self
            .groups
            .binary_search_by(|(held, _, _)| (*held).cmp(path));
        let (start, end) = match found {
            Ok(found) => (self.groups[found].1, self.groups[found].2),
            Err(after) => {
                let start = self
                    .groups
                    .get(after)
                    .map_or(self.ordered.len(), |group| group.1);
                (start, start)
            }
        };
        self.ordered.splice(start..end, sorted(definitions));
        self.groups = groups_of(&self.ordered);
    }
}

fn sorted<'a>(definitions: &[Definition<'a>]) -> Vec<Definition<'a>> {
    let mut keyed: Vec<((&'a str, u32, String), Definition<'a>)> = (definitions.iter())
        .map(|entry| {
            let key = (entry.path(), entry.symbol().line, entry.symbol().qualname());
            (key, *entry)
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.into_iter().map(|(_, entry)| entry).collect()
}

fn groups_of<'a>(ordered: &[Definition<'a>]) -> Vec<(&'a str, usize, usize)> {
    let mut groups: Vec<(&'a str, usize, usize)> = Vec::new();
    for (position, entry) in ordered.iter().enumerate() {
        match groups.last_mut() {
            Some((path, _, end)) if *path == entry.path() => *end = position + 1,
            _ => groups.push((entry.path(), position, position + 1)),
        }
    }
    groups
}
