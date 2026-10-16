//! The one place a name spelled in a file becomes an evidence tier and a candidate set.

use std::collections::{BTreeSet, VecDeque};

use facts::{Ref, Role};

use crate::names::{Definition, NameIndex};
use crate::scan::FileFacts;
use crate::scopes::{ImportScope, Scopes};

// How many re-export hops one alias chain may follow; the visited set already ends a cycle.
const MAX_IMPORT_HOPS: u32 = 32;

const TIER_VALUES: [&str; 4] = ["same_file", "imported", "unique", "ambiguous"];
const TIER_LABELS: [&str; 4] = [
    "same-file",
    "resolved-via-import",
    "unique",
    "name-only-ambiguous",
];

/// How strong the evidence behind one resolution is, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    SameFile,
    Imported,
    Unique,
    Ambiguous,
}

impl Tier {
    /// Every tier, strongest first: the precedence rule.
    pub const ORDER: [Tier; 4] = [
        Tier::SameFile,
        Tier::Imported,
        Tier::Unique,
        Tier::Ambiguous,
    ];

    /// Return the wire value, such as `same_file`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        TIER_VALUES[self as usize]
    }

    /// Return the phrase a reader sees, such as `resolved-via-import`.
    #[must_use]
    pub fn label(self) -> &'static str {
        TIER_LABELS[self as usize]
    }

    /// Return true for a binding tier, same-file or resolved-via-import, whose edges count.
    #[must_use]
    pub fn binds(self) -> bool {
        matches!(self, Tier::SameFile | Tier::Imported)
    }
}

/// The tier one name resolved at, and every candidate at that tier.
#[derive(Clone, Debug)]
pub struct Resolution<'a> {
    pub tier: Tier,
    pub candidates: Vec<Definition<'a>>,
}

// The definitions of one name, ordered by (path, line, qualified name), and their path groups.
struct Candidates<'a> {
    ordered: Vec<Definition<'a>>,
    groups: Vec<(&'a str, usize, usize)>,
}

/// Resolves names against one scan's name index and import scopes.
pub struct Resolver<'a> {
    index: &'a NameIndex<'a>,
    scopes: &'a Scopes<'a>,
    candidates: Vec<Candidates<'a>>,
}

impl<'a> Resolver<'a> {
    /// Build the resolver; `index` and `scopes` must come from the same scanned files.
    ///
    /// # Panics
    ///
    /// Panics when `index` and `scopes` were built from different file slices.
    #[must_use]
    pub fn new(index: &'a NameIndex<'a>, scopes: &'a Scopes<'a>) -> Resolver<'a> {
        assert!(
            std::ptr::eq(index.files(), scopes.files()),
            "the name index and the import scopes come from different scans"
        );
        let candidates = (0..index.name_count())
            .map(|position| ordered(index.definitions_at(position)))
            .collect();
        Resolver {
            index,
            scopes,
            candidates,
        }
    }

    /// Return the name index the resolver reads.
    #[must_use]
    pub fn index(&self) -> &'a NameIndex<'a> {
        self.index
    }

    /// Return the import scopes the resolver reads.
    #[must_use]
    pub fn scopes(&self) -> &'a Scopes<'a> {
        self.scopes
    }

    /// Resolve `name` as spelled in the file at `path`, or `None` when nothing defines it.
    #[must_use]
    pub fn resolve(&self, name: &str, path: &str) -> Option<Resolution<'a>> {
        let known = self
            .index
            .position(name)
            .map(|position| &self.candidates[position]);
        if let Some(found) = known.and_then(|known| self.bound_here(known, name, path)) {
            return Some(found);
        }
        let followed = self.through_imports(name, path);
        if !followed.is_empty() {
            return Some(imported(followed));
        }
        let ordered = known.map(|known| known.ordered.clone()).unwrap_or_default();
        match ordered.len() {
            0 => None,
            1 => Some(Resolution {
                tier: Tier::Unique,
                candidates: ordered,
            }),
            _ => Some(Resolution {
                tier: Tier::Ambiguous,
                candidates: ordered,
            }),
        }
    }

    /// Follow `name`'s `from x import` bindings in the file at `path` through aliases and
    /// re-exports to the definitions they reach.
    #[must_use]
    pub fn through_imports(&self, name: &str, path: &str) -> Vec<Definition<'a>> {
        let Some(scope) = self.scopes.get(path) else {
            return Vec::new();
        };
        let first = scope.members.get(name).into_iter().flatten().copied();
        self.follow(first.collect())
    }

    /// Return each identifier occurrence in `file` that binds a definition, with its resolution;
    /// `names`, when given, keeps only the occurrences of those names.
    pub fn resolved_sites<'r>(
        &'r self,
        file: &'a FileFacts,
        names: Option<&'r BTreeSet<&str>>,
    ) -> impl Iterator<Item = (&'a Ref, Resolution<'a>)> + 'r {
        let declarations: BTreeSet<(&str, u32)> = file
            .facts
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line))
            .collect();
        file.facts.refs.iter().filter_map(move |reference| {
            let wanted = names.is_none_or(|names| names.contains(reference.name.as_str()));
            let declared = declarations.contains(&(reference.name.as_str(), reference.line));
            if !wanted || !reference.is_resolvable() || declared {
                return None;
            }
            self.resolve_site(reference, &file.path)
                .map(|found| (reference, found))
        })
    }

    fn resolve_site(&self, reference: &Ref, path: &str) -> Option<Resolution<'a>> {
        if reference.role == Role::ModuleAttribute {
            self.resolve_module_attribute(&reference.name, path, &reference.qualifier)
        } else {
            self.resolve(&reference.name, path)
        }
    }

    // A local definition outranks an import, and an import outranks a name-only match.
    fn bound_here(&self, known: &Candidates<'a>, name: &str, path: &str) -> Option<Resolution<'a>> {
        let same_file: Vec<Definition<'a>> = group(known, path)
            .iter()
            .filter(|entry| in_module_scope(entry, path))
            .copied()
            .collect();
        if !same_file.is_empty() {
            return Some(Resolution {
                tier: Tier::SameFile,
                candidates: same_file,
            });
        }
        let scope = self.scopes.get(path)?;
        let named: Vec<Definition<'a>> = groups(known)
            .filter(|(defining, _)| scope.binds(name, defining))
            .flat_map(|(_, entries)| entries.iter().copied())
            .collect();
        (!named.is_empty()).then(|| imported(named))
    }

    // `qualifier.name`, only through the repository modules `qualifier` binds in this file.
    fn resolve_module_attribute(
        &self,
        name: &str,
        path: &str,
        qualifier: &str,
    ) -> Option<Resolution<'a>> {
        let scope = self.scopes.get(path)?;
        let targets = scope.module_bindings.get(qualifier);
        let bound = |defining: &str| targets.is_some_and(|targets| targets.contains(defining));
        let known = self
            .index
            .position(name)
            .map(|position| &self.candidates[position]);
        let direct: Vec<Definition<'a>> = known
            .into_iter()
            .flat_map(groups)
            .filter(|(defining, _)| bound(defining))
            .flat_map(|(_, entries)| entries.iter().copied())
            .collect();
        let candidates = if direct.is_empty() {
            let first = targets.into_iter().flatten().map(|module| (*module, name));
            self.follow(first.collect())
        } else {
            direct
        };
        (!candidates.is_empty()).then(|| imported(candidates))
    }

    // Breadth first over (file, member) hops; the first definition at each (path, line) stays.
    fn follow<'n>(&self, first: Vec<(&'a str, &'n str)>) -> Vec<Definition<'a>>
    where
        'a: 'n,
    {
        let mut pending: VecDeque<(&'a str, &'n str, u32)> = first
            .into_iter()
            .map(|(target, member)| (target, member, 1))
            .collect();
        let mut visited: BTreeSet<(&'a str, &'n str)> = BTreeSet::new();
        let mut found = Found::default();
        while let Some((target, member, depth)) = pending.pop_front() {
            if !visited.insert((target, member)) {
                continue;
            }
            let defined = self.defined_at(member, target);
            found.extend(&defined);
            if !defined.is_empty() || depth >= MAX_IMPORT_HOPS {
                continue;
            }
            if let Some(onward) = self.scopes.get(target) {
                let hops = re_exports(onward, member).into_iter();
                pending.extend(hops.map(|(next, name)| (next, name, depth + 1)));
            }
        }
        ordered(&found.definitions).ordered
    }

    fn defined_at(&self, member: &str, target: &str) -> Vec<Definition<'a>> {
        let Some(position) = self.index.position(member) else {
            return Vec::new();
        };
        group(&self.candidates[position], target)
            .iter()
            .filter(|entry| in_module_scope(entry, target))
            .copied()
            .collect()
    }
}

#[derive(Default)]
struct Found<'a> {
    keys: BTreeSet<(&'a str, u32)>,
    definitions: Vec<Definition<'a>>,
}

impl<'a> Found<'a> {
    fn extend(&mut self, defined: &[Definition<'a>]) {
        for entry in defined {
            if self.keys.insert((entry.path(), entry.symbol().line)) {
                self.definitions.push(*entry);
            }
        }
    }
}

fn imported(candidates: Vec<Definition<'_>>) -> Resolution<'_> {
    Resolution {
        tier: Tier::Imported,
        candidates,
    }
}

// An explicit import of the name outranks a star import, and a star import carries no private name.
fn re_exports<'a, 'n>(scope: &ImportScope<'a>, member: &'n str) -> Vec<(&'a str, &'n str)>
where
    'a: 'n,
{
    if let Some(explicit) = scope.members.get(member).filter(|pairs| !pairs.is_empty()) {
        return explicit.iter().map(|&(path, name)| (path, name)).collect();
    }
    if member.starts_with('_') {
        return Vec::new();
    }
    scope
        .wholesale
        .iter()
        .map(|&module| (module, member))
        .collect()
}

// A bare name cannot reach a class member, so only a top-level definition is in module scope.
fn in_module_scope(candidate: &Definition<'_>, path: &str) -> bool {
    candidate.path() == path && candidate.symbol().parent.is_empty()
}

fn ordered<'a>(definitions: &[Definition<'a>]) -> Candidates<'a> {
    let mut keyed: Vec<((&'a str, u32, String), Definition<'a>)> = definitions
        .iter()
        .map(|entry| {
            (
                (entry.path(), entry.symbol().line, entry.symbol().qualname()),
                *entry,
            )
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    let ordered: Vec<Definition<'a>> = keyed.into_iter().map(|(_, entry)| entry).collect();
    let mut groups: Vec<(&'a str, usize, usize)> = Vec::new();
    for (position, entry) in ordered.iter().enumerate() {
        match groups.last_mut() {
            Some((path, _, end)) if *path == entry.path() => *end = position + 1,
            _ => groups.push((entry.path(), position, position + 1)),
        }
    }
    Candidates { ordered, groups }
}

fn group<'c, 'a>(known: &'c Candidates<'a>, path: &str) -> &'c [Definition<'a>] {
    known
        .groups
        .binary_search_by(|(held, _, _)| (*held).cmp(path))
        .map_or(&[], |found| {
            let (_, start, end) = known.groups[found];
            &known.ordered[start..end]
        })
}

fn groups<'c, 'a>(
    known: &'c Candidates<'a>,
) -> impl Iterator<Item = (&'a str, &'c [Definition<'a>])> {
    known
        .groups
        .iter()
        .map(|&(path, start, end)| (path, &known.ordered[start..end]))
}
