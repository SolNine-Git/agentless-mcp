//! The resolution of a name the caller typed to the definitions it matches, best match first,
//! which explain and refs read, and the endpoint that path starts from.

use graph::{Definition, Lookup, NameIndex, SymbolGraph, definitions_for, resolve_definitions};
use prompts::messages::{PATH_DATA_KEY_NO_EDGES, PATH_ENDPOINT_NO_MATCH};

use crate::span::data_key_span;
use crate::{Noted, Reading};

// The definitions `target` names, best first. A dotted name that no file path confirms still
// names the definitions whose qualified name it spells, such as `Class.method`.
pub(crate) fn resolve_target<'a>(index: &NameIndex<'a>, target: &str) -> Lookup<'a> {
    let lookup = resolve_definitions(index, target);
    let mut ranked = rank_candidates(lookup.definitions, target);
    let exact = exact_count(&ranked, target);
    let named = !lookup.scoped && exact > 0;
    if named {
        ranked.truncate(exact);
    }
    Lookup {
        definitions: ranked,
        scoped: lookup.scoped || named,
    }
}

// A definition whose qualified name is the text the caller typed outranks one that only ends
// with it; inside a band, the path and the line decide.
pub(crate) fn rank_candidates<'a>(
    definitions: Vec<Definition<'a>>,
    target: &str,
) -> Vec<Definition<'a>> {
    let mut ranked = definitions;
    ranked.sort_by_key(|definition| {
        let inexact = definition.symbol().qualname() != target;
        (inexact, definition.path(), definition.symbol().line)
    });
    ranked
}

// The exact qualified-name matches of `ranked`, or all of it when none is exact.
pub(crate) fn best_band<'r, 'a>(
    ranked: &'r [Definition<'a>],
    target: &str,
) -> &'r [Definition<'a>] {
    let exact = exact_count(ranked, target);
    if exact == 0 { ranked } else { &ranked[..exact] }
}

// How many definitions at the head of `ranked` have `target` as their qualified name.
fn exact_count(ranked: &[Definition<'_>], target: &str) -> usize {
    ranked
        .iter()
        .take_while(|definition| definition.symbol().qualname() == target)
        .count()
}

// One endpoint the caller named: the node it resolved to and its label, or why it resolved to
// none.
pub(crate) struct Located {
    pub(crate) node: String,
    pub(crate) label: String,
    pub(crate) message: String,
}

// A stable id and a path are exact. A bare name resolves only when its best band holds one
// symbol: picking one of several would put a guess at the end of a path that reads as evidence.
pub(crate) fn endpoint(index: &NameIndex<'_>, graph: &SymbolGraph<'_>, text: &str) -> Located {
    if let Some(definition) = graph.definitions().get(text) {
        return located(text.to_owned(), definition.symbol().qualname());
    }
    if graph.has_file(text) {
        return located(text.to_owned(), text.to_owned());
    }
    let candidates = definitions_for(index, text);
    if candidates.is_empty() {
        return refused(text, PATH_ENDPOINT_NO_MATCH.render(text));
    }
    let ranked = rank_candidates(candidates, text);
    match best_band(&ranked, text) {
        [only] => located(only.stable_id().to_string(), only.symbol().qualname()),
        best => {
            let ids: Vec<String> = best
                .iter()
                .map(|entry| entry.stable_id().to_string())
                .collect();
            refused(text, format!("{text} is ambiguous: {}", ids.join(", ")))
        }
    }
}

// A data key stays out of the graph, so a data file's id is no endpoint: the key has no edges, or
// the file does not hold it and the reason is the one expand gives. `None` for any other text.
pub(crate) fn data_key_endpoint(reading: &Reading<'_>, text: &str) -> Option<Noted<Located>> {
    let found = data_key_span(reading, text)?;
    Some(found.map(|span| {
        let message = match span {
            Ok(span) => PATH_DATA_KEY_NO_EDGES.render((text, span.symbol.name.as_str())),
            Err(missed) => missed.reason(),
        };
        refused(text, message)
    }))
}

fn located(node: String, label: String) -> Located {
    Located {
        node,
        label,
        message: String::new(),
    }
}

fn refused(text: &str, message: String) -> Located {
    Located {
        node: String::new(),
        label: text.to_owned(),
        message,
    }
}

#[cfg(test)]
pub(crate) mod scanned;
#[cfg(test)]
mod tests;
