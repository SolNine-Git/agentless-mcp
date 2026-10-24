//! `shadowing`: a module-level definition the patch adds over a name the file already defines.

use std::collections::BTreeMap;

use checks::{Finding, Severity};
use facts::Symbol;
use graph::FileFacts;

use crate::fragment::{Fragment, PatchNames, unscanned_gaps};
use crate::repo::PatchBaseline;
use crate::{CHECK_SHADOWING, quoted};

pub(crate) fn check(
    fragments: &[Fragment<'_>],
    facts: &PatchBaseline<'_>,
    names: &PatchNames<'_>,
) -> Vec<Finding> {
    let mut findings = unscanned_gaps(CHECK_SHADOWING, fragments, facts);
    for fragment in fragments {
        let existing = module_level_symbols(facts.file(fragment.path()));
        // Any edit to the file may hold the definition being replaced: a move deletes it in one
        // block and writes it in another.
        let replaced = names.replaced_in(fragment.path());
        let shadowing = fragment.introduced.iter().filter_map(|symbol| {
            let replacing = replaced.is_some_and(|names| names.contains(symbol.name.as_str()));
            if replacing || !fragment.is_module_level(symbol) {
                return None;
            }
            let previous = existing.get(symbol.name.as_str())?;
            Some(shadowing_finding(fragment, symbol, previous))
        });
        findings.extend(shadowing);
    }
    findings
}

// The file's top-level symbols by name, the first definition winning.
fn module_level_symbols(file: Option<&FileFacts>) -> BTreeMap<&str, &Symbol> {
    let mut indexed = BTreeMap::new();
    let symbols = file.into_iter().flat_map(|file| file.facts.symbols.iter());
    for symbol in symbols.filter(|symbol| symbol.parent.is_empty()) {
        indexed.entry(symbol.name.as_str()).or_insert(symbol);
    }
    indexed
}

fn shadowing_finding(fragment: &Fragment<'_>, symbol: &Symbol, previous: &Symbol) -> Finding {
    Finding {
        check: CHECK_SHADOWING,
        severity: Severity::Warning,
        message: format!(
            "the patch adds a module-level {} named {} to a file that already defines \
             that name; the later definition wins at import time",
            symbol.kind.as_str(),
            quoted(&symbol.name)
        ),
        path: fragment.path().to_owned(),
        line: fragment.file_line(symbol.line),
        evidence: format!(
            "existing {} at {}:{}",
            previous.kind.as_str(),
            fragment.path(),
            previous.line
        ),
    }
}
