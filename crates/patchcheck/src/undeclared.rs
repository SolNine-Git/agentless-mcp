//! `undeclared_imports`: a top-level package the patch imports that no manifest declares, the
//! standard library does not hold, and no repository file already imports.

use std::collections::BTreeSet;

use checks::{Finding, Severity};
use facts::Import;
use graph::ImportTargets;
use syntax::Language;

use crate::fragment::{Fragment, language_gaps};
use crate::interpreter::is_stdlib_module;
use crate::manifest::{DeclaredDependencies, normalize_distribution};
use crate::repo::RepoFacts;
use crate::{CHECK_UNDECLARED_IMPORTS, MAX_DUPLICATE_SITES, gap, more, quoted};

pub(crate) fn check(fragments: &[Fragment<'_>], facts: &RepoFacts<'_>) -> Vec<Finding> {
    let mut findings = language_gaps(
        CHECK_UNDECLARED_IMPORTS,
        fragments,
        "declared-dependency manifest",
    );
    let checkable: Vec<&Fragment<'_>> = fragments
        .iter()
        .filter(|fragment| fragment.language == Language::Python)
        .collect();
    if checkable.is_empty() {
        return findings;
    }
    let declared = facts.dependencies();
    if !declared.known() {
        findings.push(no_manifest_gap(declared));
        return findings;
    }
    let accused = accusations(&checkable, facts);
    if !accused.is_empty() {
        findings.extend(unmapped_gap(declared));
    }
    findings.extend(accused);
    findings
}

// One finding per file and package that nothing makes available.
fn accusations(checkable: &[&Fragment<'_>], facts: &RepoFacts<'_>) -> Vec<Finding> {
    let available = available_packages(facts);
    let targets = ImportTargets::build(facts.files().map(|file| file.path.as_str()));
    let mut seen = BTreeSet::new();
    let mut accused = Vec::new();
    for fragment in checkable {
        for import in &fragment.imports {
            let top = top_level_package(import);
            if top.is_empty() || available.contains(&normalize_distribution(top)) {
                continue;
            }
            if targets.resolve(fragment.path(), import).is_some() {
                continue;
            }
            if seen.insert((fragment.path(), top)) {
                accused.push(undeclared_finding(fragment, import, top));
            }
        }
    }
    accused
}

// The distributed package an import names, or empty for a relative or path-shaped import and for
// a standard-library module.
fn top_level_package(import: &Import) -> &str {
    let module = text::strip(&import.module);
    if import.is_relative || module.is_empty() || module.starts_with(['.', '/']) {
        return "";
    }
    let top = module.split('.').next().unwrap_or_default();
    let top = top.split('/').next().unwrap_or_default();
    if top.is_empty() || is_stdlib_module(top) {
        return "";
    }
    top
}

// The declared distributions, and every top-level package a repository file already imports:
// that the repository runs is the evidence that it installs. No installed distribution is read.
fn available_packages(facts: &RepoFacts<'_>) -> BTreeSet<String> {
    let imported = facts
        .files()
        .flat_map(|file| file.facts.imports.iter())
        .map(top_level_package)
        .filter(|top| !top.is_empty())
        .map(normalize_distribution);
    facts
        .dependencies()
        .packages
        .iter()
        .cloned()
        .chain(imported)
        .collect()
}

fn no_manifest_gap(declared: &DeclaredDependencies) -> Finding {
    if declared.warnings.is_empty() {
        return gap(
            CHECK_UNDECLARED_IMPORTS,
            "",
            "not checked: the repository has no pyproject.toml or requirements file".to_owned(),
            "no dependency manifest found".to_owned(),
        );
    }
    gap(
        CHECK_UNDECLARED_IMPORTS,
        "",
        "not checked: no dependency manifest here could be read".to_owned(),
        declared.warnings.join("; "),
    )
}

// No distribution is installed in the binary's environment, so every declared one is unmapped.
fn unmapped_gap(declared: &DeclaredDependencies) -> Option<Finding> {
    if declared.packages.is_empty() {
        return None;
    }
    let listed: Vec<&str> = declared
        .packages
        .iter()
        .take(MAX_DUPLICATE_SITES)
        .map(String::as_str)
        .collect();
    Some(gap(
        CHECK_UNDECLARED_IMPORTS,
        "",
        format!(
            "partially checked: {} declared distribution(s) are not installed \
             in this environment, so the import names they provide are unknown \
             and an import below may be one of them",
            declared.packages.len()
        ),
        format!(
            "not installed here: {}{}",
            listed.join(", "),
            more(declared.packages.len(), MAX_DUPLICATE_SITES)
        ),
    ))
}

fn undeclared_finding(fragment: &Fragment<'_>, import: &Import, top: &str) -> Finding {
    Finding {
        check: CHECK_UNDECLARED_IMPORTS,
        severity: Severity::Warning,
        message: format!(
            "the patch imports {}, which is neither a declared dependency, \
             nor part of the standard library, \
             nor imported anywhere else in this repository",
            quoted(top)
        ),
        path: fragment.path().to_owned(),
        line: fragment.file_line(import.line),
        evidence: format!("import of module {}", quoted(&import.module)),
    }
}
