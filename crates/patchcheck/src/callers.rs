//! `dangling_callers`: a symbol the patch removes or renames that untouched files still reference.

use std::collections::BTreeSet;

use checks::{Finding, Severity};
use graph::FileFacts;

use crate::fragment::{Fragment, PatchNames, unscanned_gaps};
use crate::repo::RepoFacts;
use crate::{CHECK_DANGLING_CALLERS, more, quoted};

const MAX_CALLER_SITES: usize = 5;

pub(crate) fn check(
    fragments: &[Fragment<'_>],
    facts: &RepoFacts<'_>,
    names: &PatchNames<'_>,
) -> Vec<Finding> {
    let mut findings = unscanned_gaps(CHECK_DANGLING_CALLERS, fragments, facts);
    let touched: BTreeSet<&str> = fragments.iter().map(Fragment::path).collect();
    let untouched: Vec<&FileFacts> = facts
        .files()
        .filter(|file| !touched.contains(file.path.as_str()))
        .collect();
    let defined: BTreeSet<&str> = untouched
        .iter()
        .flat_map(|file| file.facts.symbols.iter())
        .map(|symbol| symbol.name.as_str())
        .collect();
    let mut reported = BTreeSet::new();
    for fragment in fragments {
        // Per file: a definition moved to another module leaves this one's importers dangling.
        let Some(removed) = names.removed_in(fragment.path()) else {
            continue;
        };
        let gone = fragment
            .replaced_names
            .iter()
            .filter(|name| removed.contains(name.as_str()));
        for name in gone {
            if reported.contains(name) || defined.contains(name.as_str()) {
                continue;
            }
            let sites = reference_sites(name, &untouched);
            if sites.is_empty() {
                continue;
            }
            reported.insert(name);
            findings.push(caller_finding(fragment, name, &sites));
        }
    }
    findings
}

// The `path:line` of every reference to `name`; a local of the same spelling is not one.
fn reference_sites(name: &str, files: &[&FileFacts]) -> Vec<String> {
    files
        .iter()
        .flat_map(|file| {
            file.facts
                .refs
                .iter()
                .filter(|reference| reference.name == name && reference.is_reference())
                .map(|reference| format!("{}:{}", file.path, reference.line))
        })
        .collect()
}

fn caller_finding(fragment: &Fragment<'_>, name: &str, sites: &[String]) -> Finding {
    let listed = sites[..sites.len().min(MAX_CALLER_SITES)].join(", ");
    Finding {
        check: CHECK_DANGLING_CALLERS,
        severity: Severity::Warning,
        message: format!(
            "the patch removes or renames {}, which {} reference(s) in files \
             this patch does not touch still name",
            quoted(name),
            sites.len()
        ),
        path: fragment.path().to_owned(),
        line: fragment.base_line(),
        evidence: format!(
            "still referenced at {listed}{}",
            more(sites.len(), MAX_CALLER_SITES)
        ),
    }
}
