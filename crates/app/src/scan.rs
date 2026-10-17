//! One call's scan: every file the walk lists in a language, read once and turned into facts by
//! the fact source, in walk order, with the files it passed over and why.

use std::path::Path;

use facts::{Facts, Symbol};
use git::RepoFile;
use graph::FileFacts;
use store::{FileText, Served};
use syntax::{Language, Unparsable};

use crate::detached::answered_over;
use crate::parallel::{map_in_order, threads};
use crate::{Error, Noted, Reading, Services};

/// A file the scan passed over, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The path below the repository root.
    pub path: String,
    /// Why the scan passed over it.
    pub reason: String,
}

// The scan of one repository: the files with facts, and the files passed over, in walk order.
#[derive(Debug, Default)]
pub(crate) struct RepoFacts {
    pub(crate) files: Vec<FileFacts>,
    pub(crate) skipped: Vec<Skipped>,
}

// The scan that reads each file's symbols and no other fact.
#[derive(Debug, Default)]
pub(crate) struct SymbolFacts {
    pub(crate) files: Vec<FileSymbols>,
    pub(crate) skipped: Vec<Skipped>,
}

#[derive(Debug)]
pub(crate) struct FileSymbols {
    pub(crate) path: String,
    pub(crate) language: Language,
    pub(crate) symbols: Vec<Symbol>,
}

// The facts of one file's text, and the warning of a cache row that a parse replaced.
type FactsOf<'f> = dyn Fn(&FileText) -> Served<Result<Facts, Unparsable>> + Sync + 'f;

// One file's scanned value, as `FactsOf` gives the facts.
type ValueOf<'f, T> = dyn Fn(&FileText) -> Served<Result<T, Unparsable>> + Sync + 'f;

impl RepoFacts {
    // The walk's warnings and each unreadable cache row's warning come with either outcome.
    pub(crate) fn load(
        services: &Services,
        reading: &Reading<'_>,
    ) -> Noted<Result<RepoFacts, Error>> {
        let root = &reading.repo.root;
        let facts_of = |text: &FileText| reading.facts.facts(text);
        walked(services, root, |files| {
            scan(root, files, threads(), &facts_of)
        })
    }
}

// Scans the repository once and gives the scan to `answer`. The scan's warnings come with
// either outcome.
pub(crate) fn over_scan<T>(
    services: &Services,
    reading: &Reading<'_>,
    answer: impl FnOnce(&RepoFacts) -> Result<T, Error>,
) -> Noted<Result<T, Error>> {
    RepoFacts::load(services, reading)
        .map(|loaded| loaded.and_then(|facts| answered_over(facts, answer)))
}

impl SymbolFacts {
    // The warnings come as they come with `RepoFacts::load`.
    pub(crate) fn load(
        services: &Services,
        reading: &Reading<'_>,
    ) -> Noted<Result<SymbolFacts, Error>> {
        let root = &reading.repo.root;
        let symbols_of = |text: &FileText| {
            let Served { value, warning } = reading.facts.symbols(text);
            let value = value.map(|symbols| FileSymbols {
                path: text.path.clone(),
                language: text.language,
                symbols,
            });
            Served { value, warning }
        };
        walked(services, root, |files| {
            let scanned = scan_values(root, files, threads(), &symbols_of);
            scanned.map(|(files, skipped)| SymbolFacts { files, skipped })
        })
    }
}

// Walks the repository and scans the files it lists. A walk refusal is the outcome's error.
fn walked<T>(
    services: &Services,
    root: &Path,
    scan: impl FnOnce(Vec<RepoFile>) -> Noted<T>,
) -> Noted<Result<T, Error>> {
    let walk = services.git.walk(root);
    let mut diagnostics: Vec<String> = walk.warnings.iter().map(ToString::to_string).collect();
    let files = match walk.files {
        Ok(files) => files,
        Err(refusal) => {
            let error = kernel::Error::from(refusal).into();
            return Noted {
                value: Err(error),
                diagnostics,
            };
        }
    };
    let scanned = scan(files);
    diagnostics.extend(scanned.diagnostics);
    Noted {
        value: Ok(scanned.value),
        diagnostics,
    }
}

// Reads and parses the files in a language on `threads` threads, and keeps the walk order.
fn scan(
    root: &Path,
    files: Vec<RepoFile>,
    threads: usize,
    facts_of: &FactsOf<'_>,
) -> Noted<RepoFacts> {
    let file_facts = |text: &FileText| {
        let Served { value, warning } = facts_of(text);
        let value = value.map(|facts| FileFacts {
            path: text.path.clone(),
            language: text.language,
            line_count: line_count(&text.text),
            facts,
        });
        Served { value, warning }
    };
    let scanned = scan_values(root, files, threads, &file_facts);
    scanned.map(|(files, skipped)| RepoFacts { files, skipped })
}

// Each file's value in walk order, and the files passed over.
fn scan_values<T: Send>(
    root: &Path,
    files: Vec<RepoFile>,
    threads: usize,
    value_of: &ValueOf<'_, T>,
) -> Noted<(Vec<T>, Vec<Skipped>)> {
    let wanted: Vec<(RepoFile, Language)> = files.into_iter().filter_map(in_language).collect();
    let scanned = map_in_order(&wanted, threads, |(file, language)| {
        scan_one(root, file, *language, value_of)
    });
    let mut values = Noted::new((Vec::new(), Vec::new()));
    for Served { value, warning } in scanned {
        values
            .diagnostics
            .extend(warning.map(|failure| failure.to_string()));
        match value {
            Ok(file) => values.value.0.push(file),
            Err(skipped) => values.value.1.push(skipped),
        }
    }
    values
}

// The project config file is never scanned, and a path whose suffix names no language is not
// scanned.
fn in_language(file: RepoFile) -> Option<(RepoFile, Language)> {
    if file.path == io::CONFIG_FILENAME {
        return None;
    }
    let language = Language::from_path(&file.path)?;
    Some((file, language))
}

fn scan_one<T>(
    root: &Path,
    file: &RepoFile,
    language: Language,
    value_of: &ValueOf<'_, T>,
) -> Served<Result<T, Skipped>> {
    let bytes = match io::read_bounded(&root.join(&file.path)) {
        Ok(bytes) => bytes,
        Err(skipped) => {
            return Served {
                value: Err(skip(&file.path, &skipped)),
                warning: None,
            };
        }
    };
    let text = FileText::decode(file.path.clone(), language, &bytes);
    let Served { value, warning } = value_of(&text);
    let value = value.map_err(|unparsable| skip(&file.path, &unparsable));
    Served { value, warning }
}

fn skip(path: &str, reason: &impl ToString) -> Skipped {
    Skipped {
        path: path.to_owned(),
        reason: reason.to_string(),
    }
}

// The scan counts the parts that "\n" splits the text into, so a final newline adds a line.
fn line_count(text: &str) -> u32 {
    u32::try_from(text.split('\n').count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
