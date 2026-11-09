//! A Rust import that takes from a module file only names that the file imports in turn, such as
//! `use crate::Error` against `pub use error::Error`, is a cycle edge to the defining files.

use std::collections::BTreeSet;

use syntax::Language;

use crate::scopes::Scopes;

// The most files that one name's chain of re-exports passes through before the import keeps its
// first target.
const REEXPORT_HOPS: usize = 4;

// `None` keeps the import of `target`: `source` takes the module, a glob or a name that `target`
// defines, or a leaf of one of its statements binds no name.
pub(crate) fn defining_files<'a>(
    scopes: &Scopes<'a>,
    source: &str,
    target: &'a str,
) -> Option<BTreeSet<&'a str>> {
    let rust = |path: &str| scopes.language_of(path) == Some(Language::Rust);
    if !rust(source) || !rust(target) {
        return None;
    }
    let scope = scopes.get(source)?;
    let whole = scope.wholesale.contains(target)
        || (scope.module_bindings.values()).any(|targets| targets.contains(target));
    if whole || !every_leaf_binds(scopes, source, target) {
        return None;
    }
    let taken: BTreeSet<&'a str> = (scope.members.values().flatten())
        .filter(|(file, _)| *file == target)
        .map(|(_, member)| *member)
        .collect();
    if taken.is_empty() {
        return None;
    }
    let mut defining = BTreeSet::new();
    for member in taken {
        defining.extend(origins(scopes, target, member)?);
    }
    defining.remove(source);
    Some(defining)
}

// A leaf such as `_`, or a path that goes past an item, binds no name and still reaches the file.
fn every_leaf_binds(scopes: &Scopes<'_>, source: &str, target: &str) -> bool {
    let (Some(scope), Some(position)) = (scopes.get(source), scopes.position(source)) else {
        return false;
    };
    let imports = &scopes.files()[position].facts.imports;
    (scope.statements.iter())
        .filter(|row| row.resolved == Some(target))
        .all(|row| {
            (imports.get(row.statement)).is_some_and(|import| {
                (import.bound_names()).all(|(_, local)| scope.named.contains_key(local))
            })
        })
}

// The files that define `member` of `file` through its imports; `None` when `file` defines it
// itself, or when the chain is longer than the bound.
fn origins<'a>(scopes: &Scopes<'a>, file: &'a str, member: &'a str) -> Option<BTreeSet<&'a str>> {
    let imported = |file: &str, member: &str| {
        let scope = scopes.get(file)?;
        scope
            .members
            .get(member)
            .filter(|origins| !origins.is_empty())
    };
    let mut frontier: Vec<(&'a str, &'a str)> = imported(file, member)?.iter().copied().collect();
    let mut defining = BTreeSet::new();
    for _ in 0..REEXPORT_HOPS {
        let mut next = Vec::new();
        for (file, member) in frontier {
            match imported(file, member) {
                Some(further) => next.extend(further.iter().copied()),
                None => {
                    defining.insert(file);
                }
            }
        }
        if next.is_empty() {
            return Some(defining);
        }
        frontier = next;
    }
    None
}
