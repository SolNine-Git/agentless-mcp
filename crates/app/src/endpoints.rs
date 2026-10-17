//! The resolution of a name the caller typed to the definitions it matches, best match first,
//! and the endpoint that explain and path start from.

use graph::{Definition, NameIndex, SymbolGraph, definitions_for};

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
    let exact = ranked
        .iter()
        .take_while(|definition| definition.symbol().qualname() == target)
        .count();
    if exact == 0 { ranked } else { &ranked[..exact] }
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
        return refused(text, format!("no symbol or file matches {text}"));
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
