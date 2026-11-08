//! The one place a name spelled in a file becomes an evidence tier and a candidate set.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use facts::{Declared, Ref, Role};
use syntax::Language;

use crate::candidates::Candidates;
use crate::names::{Definition, NameIndex};
use crate::scan::FileFacts;
use crate::scopes::{ImportScope, Qualified, Scopes};

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

// How a site spells its name: a bare name reads only the file and the name, a qualified name and a
// module attribute read their qualifier as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Shape {
    Bare,
    Qualified,
    ModuleAttribute,
}

// Everything one resolution reads: the scan position of the spelling file, the shape, the
// qualifier (empty for a bare name) and the name.
#[derive(Clone, Copy)]
struct Spelled<'s> {
    at: Option<usize>,
    shape: Shape,
    qualifier: &'s str,
    name: &'s str,
}

impl<'s> Spelled<'s> {
    fn bare(at: Option<usize>, name: &'s str) -> Spelled<'s> {
        Spelled {
            at,
            shape: Shape::Bare,
            qualifier: "",
            name,
        }
    }
}

// A spelling's file, shape and name position in the index; the qualifier keys the list below it.
type Key = (usize, Shape, usize);

// A resolution kept without the scan's lifetime, so the resolver stays covariant: the tier and the
// (file, symbol) position of each candidate.
struct Kept {
    tier: Tier,
    at: Vec<(usize, usize)>,
}

// Each key with every qualifier it was resolved under, and what it resolved to.
type Memo = HashMap<Key, Vec<(Box<str>, Option<Kept>)>>;

/// The resolutions that a resolver kept, apart from the parts it read.
pub struct Answers {
    _held: Memo,
}

/// Resolves names against one scan's name index and import scopes, which it borrows for `'r`; what
/// it resolves to lives as long as the scan, `'a`.
pub struct Resolver<'r, 'a> {
    index: &'r NameIndex<'a>,
    scopes: &'r Scopes<'a>,
    // The index and the scopes never change, so a key and qualifier resolve once for the life of
    // the resolver. A lock, not a cell, keeps the resolver `Sync` for the types that hold it.
    resolved: Mutex<Memo>,
    walks: AtomicUsize,
}

impl<'r, 'a> Resolver<'r, 'a> {
    /// Build the resolver; `index` and `scopes` must come from the same scanned files.
    ///
    /// # Panics
    ///
    /// Panics when `index` and `scopes` were built from different file slices.
    #[must_use]
    pub fn new(index: &'r NameIndex<'a>, scopes: &'r Scopes<'a>) -> Resolver<'r, 'a> {
        assert!(
            std::ptr::eq(index.files(), scopes.files()),
            "the name index and the import scopes come from different scans"
        );
        Resolver {
            index,
            scopes,
            resolved: Mutex::new(HashMap::new()),
            walks: AtomicUsize::new(0),
        }
    }

    /// Take what the resolver resolved, which outlives the parts it borrowed, for a caller that
    /// frees it elsewhere; the resolver resolves again what it needs.
    #[must_use]
    pub fn take_answers(&self) -> Answers {
        Answers {
            _held: std::mem::take(&mut *self.memo()),
        }
    }

    // How many alias chains the resolver walked: the memo holds it to one per key it resolved.
    #[cfg(test)]
    pub(crate) fn walks(&self) -> usize {
        self.walks.load(Ordering::Relaxed)
    }

    /// Return the name index the resolver reads.
    #[must_use]
    pub fn index(&self) -> &'r NameIndex<'a> {
        self.index
    }

    /// Return the import scopes the resolver reads.
    #[must_use]
    pub fn scopes(&self) -> &'r Scopes<'a> {
        self.scopes
    }

    /// Resolve `name` as spelled in the file at `path`, or `None` when nothing defines it; a
    /// name-only match keeps the definitions in the language family of the file at `path`.
    #[must_use]
    pub fn resolve(&self, name: &str, path: &str) -> Option<Resolution<'a>> {
        let at = self.scopes.position(path);
        self.memoized(Spelled::bare(at, name), || self.resolve_bare(name, path))
    }

    fn resolve_bare(&self, name: &str, path: &str) -> Option<Resolution<'a>> {
        let known = self.index.candidates(name);
        if let Some(found) = known.and_then(|known| self.bound_here(known, name, path)) {
            return Some(found);
        }
        let followed = self.through_imports(name, path);
        if !followed.is_empty() {
            return Some(imported(followed));
        }
        let mut ordered = known.map(|known| known.ordered.clone()).unwrap_or_default();
        if let Some(spelled) = self.scopes.language_of(path) {
            ordered.retain(|entry| entry.file().language.family() == spelled.family());
            ordered.retain(|entry| bare_name_reaches(Some(spelled), entry));
        }
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
        let mut first: Vec<(&'a str, &str)> = scope
            .members
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        // A Rust glob import also brings in what the globbed module re-exports.
        if first.is_empty() && self.scopes.language_of(path) == Some(Language::Rust) {
            first = scope
                .wholesale
                .iter()
                .map(|&module| (module, name))
                .collect();
        }
        self.follow(first)
    }

    /// Return each identifier occurrence in `file` that binds a definition, with its resolution;
    /// `names`, when given, keeps only the occurrences of those names.
    pub fn resolved_sites<'s>(
        &'s self,
        file: &'a FileFacts,
        names: Option<&'s BTreeSet<&str>>,
    ) -> impl Iterator<Item = (&'a Ref, Resolution<'a>)> + 's {
        let refs = file.facts.refs.iter().enumerate();
        let wanted = refs.filter(move |(_, reference)| {
            names.is_none_or(|names| names.contains(reference.name.as_str()))
        });
        let attempted = self.attempted(file, wanted);
        attempted.filter_map(|(_, reference, found)| Some((reference, found?)))
    }

    // Each of `refs`, the file's references with their places, that the resolver tries to bind,
    // with what it binds, if anything. Only a symbol's name line decides a declaring spelling.
    pub(crate) fn attempted<'s>(
        &'s self,
        file: &'a FileFacts,
        refs: impl Iterator<Item = (usize, &'a Ref)> + 's,
    ) -> impl Iterator<Item = (usize, &'a Ref, Option<Resolution<'a>>)> + 's {
        let declared = Declared::of(&file.facts.symbols, &[]);
        let at = self.scopes.position(&file.path);
        refs.filter_map(move |(place, reference)| {
            if !reference.is_resolvable() || declared.declares(reference) {
                return None;
            }
            let found = self.resolve_site(reference, &file.path, at);
            Some((place, reference, found))
        })
    }

    /// Return true when the module path that qualifies `reference` in the file at `path` names a
    /// module outside the repository, so the reference binds nothing.
    #[must_use]
    pub fn names_outside(&self, reference: &Ref, path: &str) -> bool {
        reference.role != Role::ModuleAttribute
            && !reference.qualifier.is_empty()
            && self.qualified(&reference.qualifier, path) == Some(Qualified::Foreign)
    }

    // `at` is the scan position of the file at `path`, which the caller looks up once per file.
    fn resolve_site(
        &self,
        reference: &Ref,
        path: &str,
        at: Option<usize>,
    ) -> Option<Resolution<'a>> {
        let (name, qualifier) = (reference.name.as_str(), reference.qualifier.as_str());
        let spelled = |shape| Spelled {
            at,
            shape,
            qualifier,
            name,
        };
        if reference.role == Role::ModuleAttribute {
            self.memoized(spelled(Shape::ModuleAttribute), || {
                self.resolve_module_attribute(name, path, qualifier)
            })
        } else if !qualifier.is_empty() {
            self.memoized(spelled(Shape::Qualified), || {
                self.resolve_qualified(name, path, qualifier)
            })
        } else {
            self.memoized(Spelled::bare(at, name), || self.resolve_bare(name, path))
        }
    }

    // The resolution of `spelled`, from the memo or from `resolve`; a path outside the scan or a
    // name outside the index has no key and resolves each time.
    fn memoized(
        &self,
        spelled: Spelled<'_>,
        resolve: impl FnOnce() -> Option<Resolution<'a>>,
    ) -> Option<Resolution<'a>> {
        let Some(key) = (spelled.at)
            .zip(self.index.position(spelled.name))
            .map(|(file, name)| (file, spelled.shape, name))
        else {
            return resolve();
        };
        {
            let resolved = self.memo();
            let mut held = resolved.get(&key).into_iter().flatten();
            if let Some((_, kept)) = held.find(|(held, _)| **held == *spelled.qualifier) {
                return kept.as_ref().map(|kept| self.restored(kept));
            }
        }
        let found = resolve();
        let kept = found.as_ref().map(|found| Kept {
            tier: found.tier,
            at: found.candidates.iter().map(Definition::position).collect(),
        });
        let mut resolved = self.memo();
        let held = resolved.entry(key).or_default();
        held.push((spelled.qualifier.into(), kept));
        found
    }

    // No code panics while it holds the lock, so a poisoned memo is still whole.
    fn memo(&self) -> MutexGuard<'_, Memo> {
        self.resolved.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn restored(&self, kept: &Kept) -> Resolution<'a> {
        let files = self.index.files();
        Resolution {
            tier: kept.tier,
            candidates: (kept.at.iter())
                .map(|&(file, symbol)| Definition::new(files, file, symbol))
                .collect(),
        }
    }

    fn qualified(&self, qualifier: &str, path: &str) -> Option<Qualified<'a>> {
        let scope = self.scopes.get(path)?;
        scope.qualified.get(qualifier).copied()
    }

    // `module::name` binds through the module's file and its re-exports; a module outside the
    // repository binds nothing; a path whose walk stopped inside it falls back to the name.
    fn resolve_qualified(&self, name: &str, path: &str, qualifier: &str) -> Option<Resolution<'a>> {
        let module = match self.qualified(qualifier, path) {
            Some(Qualified::Module(module)) => module,
            Some(Qualified::Foreign) => return None,
            None => return self.resolve(name, path),
        };
        let mut reached = self.defined_at(name, module);
        if reached.is_empty() {
            reached = self.follow(vec![(module, name)]);
        }
        if reached.is_empty() {
            self.resolve(name, path)
        } else {
            Some(imported(reached))
        }
    }

    // A local definition outranks an import, and an import outranks a name-only match.
    fn bound_here(&self, known: &Candidates<'a>, name: &str, path: &str) -> Option<Resolution<'a>> {
        let same_file: Vec<Definition<'a>> = (known.group(path).iter())
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
        let spelled = self.scopes.language_of(path);
        let named: Vec<Definition<'a>> = (known.groups())
            .filter(|(defining, _)| scope.binds(name, defining))
            .flat_map(|(_, entries)| entries.iter().copied())
            .filter(|entry| bare_name_reaches(spelled, entry))
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
        let known = self.index.candidates(name);
        let direct: Vec<Definition<'a>> = known
            .into_iter()
            .flat_map(Candidates::groups)
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
        self.walks.fetch_add(1, Ordering::Relaxed);
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
        Candidates::of(&found.definitions).ordered
    }

    fn defined_at(&self, member: &str, target: &str) -> Vec<Definition<'a>> {
        let Some(known) = self.index.candidates(member) else {
            return Vec::new();
        };
        (known.group(target).iter())
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

// Rust has no implicit receiver: a method is called on a value or through a type path, and neither
// binds, so a Rust bare name reaches no method at any tier.
fn bare_name_reaches(spelled: Option<Language>, candidate: &Definition<'_>) -> bool {
    spelled != Some(Language::Rust) || candidate.symbol().parent.is_empty()
}

// A bare name cannot reach a class member, so only a top-level definition is in module scope.
fn in_module_scope(candidate: &Definition<'_>, path: &str) -> bool {
    candidate.path() == path && candidate.symbol().parent.is_empty()
}
