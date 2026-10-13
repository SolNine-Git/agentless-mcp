//! What "the symbol the caller meant" is: a stable id, a dotted name or a bare name, resolved to
//! definitions, and the sites that spell a definition from outside it.

use facts::{StableId, split_ordinal};

use crate::names::{Definition, NameIndex, Site};
use crate::paths::stem;

/// The definitions a lookup target found, and whether the target named what it found.
#[derive(Clone, Debug)]
pub struct Lookup<'a> {
    pub definitions: Vec<Definition<'a>>,
    pub scoped: bool,
}

/// Resolve a stable id, a dotted name or a bare name to definitions; a target that no longer
/// names its file falls back to every definition of its last name.
#[must_use]
pub fn resolve_definitions<'a>(index: &NameIndex<'a>, target: &str) -> Lookup<'a> {
    match target.parse::<StableId>() {
        Ok(parsed) => by_stable_id(index, &parsed),
        Err(_) => by_name(index, target),
    }
}

/// Return the definitions [`resolve_definitions`] finds for `target`.
#[must_use]
pub fn definitions_for<'a>(index: &NameIndex<'a>, target: &str) -> Vec<Definition<'a>> {
    resolve_definitions(index, target).definitions
}

/// Return the sites that spell a definition's name outside its own span, in scan order.
#[must_use]
pub fn references_to<'a>(index: &NameIndex<'a>, definition: &Definition<'a>) -> Vec<Site<'a>> {
    let symbol = definition.symbol();
    let inside = |site: &Site<'a>| {
        let line = site.reference.line;
        site.path() == definition.path() && symbol.line <= line && line <= symbol.end_line
    };
    index
        .sites(&symbol.name)
        .iter()
        .filter(|site| !inside(site))
        .copied()
        .collect()
}

fn by_name<'a>(index: &NameIndex<'a>, target: &str) -> Lookup<'a> {
    let (base, _) = split_ordinal(target);
    let candidates = index.definitions(last_name(base));
    let confirmed: Vec<Definition<'a>> = candidates
        .iter()
        .filter(|definition| path_confirms(definition, base))
        .copied()
        .collect();
    if !confirmed.is_empty() {
        return Lookup {
            definitions: confirmed,
            scoped: true,
        };
    }
    // A bare name promises no file, so it cannot have missed one; a dotted name can.
    Lookup {
        definitions: candidates.to_vec(),
        scoped: !base.contains('.'),
    }
}

fn by_stable_id<'a>(index: &NameIndex<'a>, parsed: &StableId) -> Lookup<'a> {
    let (base, _) = split_ordinal(parsed.qualname());
    let all = index.definitions(last_name(base));
    let scoped: Vec<Definition<'a>> = all
        .iter()
        .filter(|definition| {
            definition.path() == parsed.path()
                && definition.symbol().id_qualname() == parsed.qualname()
        })
        .copied()
        .collect();
    let found = !scoped.is_empty();
    Lookup {
        definitions: if found { scoped } else { all.to_vec() },
        scoped: found,
    }
}

// The text after the last dot, or the whole text when that is empty.
fn last_name(base: &str) -> &str {
    base.rsplit_once('.')
        .map(|(_, last)| last)
        .filter(|last| !last.is_empty())
        .unwrap_or(base)
}

// Only a target longer than the qualified name says which file to look in.
fn path_confirms(definition: &Definition<'_>, target: &str) -> bool {
    let qualified = definition.symbol().id_qualname();
    let (base, _) = split_ordinal(&qualified);
    if target.chars().count() <= base.chars().count() {
        return false;
    }
    let dotted = format!("{}.{base}", stem(definition.path()).replace('/', "."));
    dotted == target || dotted.ends_with(&format!(".{target}"))
}
