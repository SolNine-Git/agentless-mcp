//! `cycle_delta`: import cycles that exist after the patch and did not exist before it. Both
//! graphs are built in memory; only the edited files' imports are read again.

use std::collections::{BTreeMap, BTreeSet};

use checks::{Finding, Severity};
use facts::Facts;
use graph::{Cycle, FileFacts, Scopes, SymbolGraph, import_cycles};
use patch::Edit;
use syntax::Unparsable;

use crate::fragment::{Fragment, extract};
use crate::repo::RepoFacts;
use crate::{CHECK_CYCLE_DELTA, Parse, gap};

pub(crate) fn check(
    fragments: &[Fragment<'_>],
    facts: &RepoFacts<'_>,
    parse: &Parse,
) -> Result<Vec<Finding>, Unparsable> {
    let edited: BTreeSet<&str> = fragments.iter().map(Fragment::path).collect();
    if edited.is_empty() {
        return Ok(Vec::new());
    }
    let contents = match edited_texts(&edited, facts) {
        Ok(contents) => contents,
        Err(missing) => return Ok(vec![missing]),
    };
    let edits: Vec<Edit> = fragments
        .iter()
        .map(|fragment| fragment.edit.clone())
        .collect();
    let applied = patch::apply_edits(&edits, &contents);
    if !applied.ok() {
        return Ok(vec![gap(
            CHECK_CYCLE_DELTA,
            "",
            "not checked: at least one edit did not apply, \
             so there is no post-patch tree to compare against"
                .to_owned(),
            format!("{} edit(s) did not apply", applied.failures().count()),
        )]);
    }
    let patched = patched_files(facts, &applied.new_contents, parse)?;
    let before = cycles_of(facts.scan());
    let existing: BTreeSet<BTreeSet<&str>> = before
        .iter()
        .map(|cycle| cycle.files.iter().copied().collect())
        .collect();
    Ok(cycles_of(&patched)
        .iter()
        .filter(|cycle| !existing.contains(&cycle.files.iter().copied().collect()))
        .map(cycle_finding)
        .collect())
}

// The texts of the edited files, or the gap that names the first edited file without one.
fn edited_texts(
    edited: &BTreeSet<&str>,
    facts: &RepoFacts<'_>,
) -> Result<BTreeMap<String, String>, Finding> {
    let missing: Vec<&str> = edited
        .iter()
        .copied()
        .filter(|path| facts.text(path).is_none())
        .collect();
    if let Some(first) = missing.first() {
        return Err(gap(
            CHECK_CYCLE_DELTA,
            "",
            format!(
                "not checked: no text was supplied for {} edited file(s), \
                 so the post-patch import graph could not be built",
                missing.len()
            ),
            format!("first missing: {first}"),
        ));
    }
    Ok(edited
        .iter()
        .filter_map(|path| Some(((*path).to_owned(), facts.text(path)?.to_owned())))
        .collect())
}

fn cycles_of(files: &[FileFacts]) -> Vec<Cycle<'_>> {
    import_cycles(&SymbolGraph::imports(&Scopes::build(files)))
}

// Each scanned file with only its import table, read again from the patched text for an edited
// file: the import graph reads nothing else.
fn patched_files(
    facts: &RepoFacts<'_>,
    contents: &[(String, String)],
    parse: &Parse,
) -> Result<Vec<FileFacts>, Unparsable> {
    let patched: BTreeMap<&str, &str> = contents
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    facts
        .files()
        .map(|file| {
            let imports = match patched.get(file.path.as_str()) {
                Some(text) => extract(parse, file.language, text)?.imports,
                None => file.facts.imports.clone(),
            };
            Ok(FileFacts {
                path: file.path.clone(),
                language: file.language,
                line_count: file.line_count,
                facts: Facts {
                    imports,
                    ..Facts::default()
                },
            })
        })
        .collect()
}

fn cycle_finding(cycle: &Cycle<'_>) -> Finding {
    let first = cycle.files.first().copied().unwrap_or_default();
    let chain: Vec<&str> = cycle.files.iter().copied().chain([first]).collect();
    Finding {
        check: CHECK_CYCLE_DELTA,
        severity: Severity::Warning,
        message: format!(
            "the patch introduces an import cycle across {} files that did not exist before it",
            cycle.files.len()
        ),
        path: first.to_owned(),
        line: 0,
        evidence: chain.join(" -> "),
    }
}
