//! `dangling_references`: a name the patch calls or inherits from that nothing defines or binds.

use std::collections::BTreeSet;

use checks::{Finding, Severity};
use facts::{Role, SymbolKind};
use syntax::Language;

use crate::fragment::{Fragment, PatchNames, language_gaps};
use crate::interpreter::always_bound;
use crate::lexer::signature_names;
use crate::repo::RepoFacts;
use crate::{CHECK_DANGLING_REFERENCES, quoted};

// Two edits cover a transposition, a doubled letter and a dropped one.
const MAX_EDIT_DISTANCE: usize = 2;
const MAX_NEAR_MISSES: usize = 3;

const VERB_CALLS: &str = "calls";
const VERB_INHERITS: &str = "inherits from";

// One name the patch uses in a judged position, its block line, and how it uses it.
struct Usage<'f> {
    name: &'f str,
    line: u32,
    verb: &'static str,
}

pub(crate) fn check(
    fragments: &[Fragment<'_>],
    facts: &RepoFacts<'_>,
    names: &PatchNames<'_>,
) -> Vec<Finding> {
    let mut findings = language_gaps(
        CHECK_DANGLING_REFERENCES,
        fragments,
        "builtin and keyword table",
    );
    let known = defined_names(facts);
    let python = fragments
        .iter()
        .filter(|fragment| fragment.language == Language::Python);
    for fragment in python {
        let bound = bound_names(fragment, names);
        let mut seen = BTreeSet::new();
        for usage in usages(fragment) {
            let free = !always_bound(usage.name) && !bound.contains(usage.name);
            if !free || seen.contains(usage.name) {
                continue;
            }
            if facts
                .resolver()
                .resolve(usage.name, fragment.path())
                .is_some()
            {
                continue;
            }
            seen.insert(usage.name);
            findings.push(dangling_finding(fragment, &usage, &known));
        }
    }
    findings
}

fn usages<'f>(fragment: &'f Fragment<'_>) -> Vec<Usage<'f>> {
    let calls = fragment.calls.iter().map(|site| Usage {
        name: &site.name,
        line: site.line,
        verb: VERB_CALLS,
    });
    let bases = fragment.introduced.iter().flat_map(|symbol| {
        symbol
            .bases
            .iter()
            .map(|base| facts::base_name(base))
            .filter(|name| !name.is_empty())
            .map(|name| Usage {
                name,
                line: symbol.line,
                verb: VERB_INHERITS,
            })
    });
    calls.chain(bases).collect()
}

// What the patch binds before this fragment uses it: every definition of the whole patch, the
// fragment's parameters and imports, and every occurrence the grammar scoped as a binder.
fn bound_names<'f>(fragment: &'f Fragment<'_>, names: &PatchNames<'f>) -> BTreeSet<String> {
    let mut bound: BTreeSet<String> = names
        .introduced()
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    for symbol in &fragment.introduced {
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
            bound.extend(signature_names(&symbol.signature));
        }
    }
    for import in &fragment.imports {
        bound.extend(import.names.iter().cloned());
        let top = top_module(&import.module);
        if !top.is_empty() {
            bound.insert(top.to_owned());
        }
    }
    let binders = fragment.refs.iter().filter(|reference| {
        matches!(
            reference.role,
            Role::Binding | Role::Declaration | Role::Import
        )
    });
    bound.extend(binders.map(|reference| reference.name.clone()));
    bound
}

fn top_module(module: &str) -> &str {
    let first = text::strip(module).split('.').next().unwrap_or_default();
    first.split('/').next().unwrap_or_default()
}

// Every symbol name of every scanned file, sorted and once each.
fn defined_names<'a>(facts: &RepoFacts<'a>) -> Vec<&'a str> {
    let names: BTreeSet<&str> = facts
        .files()
        .flat_map(|file| file.facts.symbols.iter())
        .map(|symbol| symbol.name.as_str())
        .collect();
    names.into_iter().collect()
}

// The existing names closest to `name`, nearest first, ties by name. A name that differs only
// in case or underscores is at distance 0.
pub(crate) fn near_misses<'k>(name: &str, known: &[&'k str]) -> Vec<&'k str> {
    let folded = fold(name);
    let mut scored: Vec<(usize, &str)> = known
        .iter()
        .filter(|candidate| **candidate != name)
        .filter_map(|&candidate| {
            if fold(candidate) == folded {
                return Some((0, candidate));
            }
            let distance = edit_distance(name, candidate, MAX_EDIT_DISTANCE);
            (distance <= MAX_EDIT_DISTANCE).then_some((distance, candidate))
        })
        .collect();
    scored.sort_unstable();
    scored
        .into_iter()
        .take(MAX_NEAR_MISSES)
        .map(|(_, candidate)| candidate)
        .collect()
}

fn fold(name: &str) -> String {
    text::lower(&name.replace('_', ""))
}

// The Levenshtein distance over code points, or `ceiling + 1` once no row can come back under it.
fn edit_distance(left: &str, right: &str, ceiling: usize) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > ceiling {
        return ceiling + 1;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.iter().enumerate() {
        let mut current = vec![row + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            let cost = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(substitution);
            current.push(cost);
        }
        if current
            .iter()
            .min()
            .is_some_and(|&cheapest| cheapest > ceiling)
        {
            return ceiling + 1;
        }
        previous = current;
    }
    previous[right.len()]
}

fn dangling_finding(fragment: &Fragment<'_>, usage: &Usage<'_>, known: &[&str]) -> Finding {
    let misses = near_misses(usage.name, known).join(", ");
    let suggestion = if misses.is_empty() {
        String::new()
    } else {
        format!("; did you mean {misses}?")
    };
    let evidence = if misses.is_empty() {
        "no existing name is within two edits of it".to_owned()
    } else {
        format!("nearest existing names: {misses}")
    };
    Finding {
        check: CHECK_DANGLING_REFERENCES,
        severity: Severity::Warning,
        message: format!(
            "the patch {} {}, which nothing in this repository defines, \
             the patch itself does not define, \
             and which is neither a builtin nor a standard library module{suggestion}",
            usage.verb,
            quoted(usage.name)
        ),
        path: fragment.path().to_owned(),
        line: fragment.file_line(usage.line),
        evidence,
    }
}
