//! One call's scan: every file the walk lists in a language, read once and turned into facts by
//! the fact source, in walk order, with the files it passed over and why.

use std::path::Path;

use facts::{Facts, Symbol};
use git::RepoFile;
use graph::FileFacts;
use kernel::{Deadline, Stopped, checkpoint};
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

// The scan that reads each file's symbols and no other fact, and where each file spells the words
// that the caller asked for.
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
    // The speller's answer for this file's text: one entry per word, in the caller's word order.
    pub(crate) spellings: Vec<Option<Spelling>>,
}

// The first line of a file that spells a word, and how many lines spell it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spelling {
    pub(crate) line: u32,
    pub(crate) lines: u32,
}

// Where one file's text spells each word that the caller asked for.
pub(crate) type Speller<'s> = dyn Fn(&FileText) -> Vec<Option<Spelling>> + Sync + 's;

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
        let facts_of = |text: &FileText| reading.facts.facts(text, reading.deadline);
        walked(services, root, |files| {
            scan(root, files, threads(), &facts_of, reading.deadline)
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
    // The warnings come as they come with `RepoFacts::load`. `spell` reads the text of each file
    // that gives symbols, on the scan threads.
    pub(crate) fn load(
        services: &Services,
        reading: &Reading<'_>,
        spell: &Speller<'_>,
    ) -> Noted<Result<SymbolFacts, Error>> {
        let root = &reading.repo.root;
        let symbols_of = |text: &FileText| {
            let Served { value, warning } = reading.facts.symbols(text, reading.deadline);
            let value = value.map(|symbols| FileSymbols {
                path: text.path.clone(),
                language: text.language,
                symbols,
                spellings: spell(text),
            });
            Served { value, warning }
        };
        walked(services, root, |files| {
            let scanned = scan_values(root, files, threads(), &symbols_of, reading.deadline);
            scanned.map(|scanned| scanned.map(|(files, skipped)| SymbolFacts { files, skipped }))
        })
    }
}

// Walks the repository and scans the files it lists. A walk refusal or a stop of the call is the
// outcome's error.
fn walked<T>(
    services: &Services,
    root: &Path,
    scan: impl FnOnce(Vec<RepoFile>) -> Noted<Result<T, Stopped>>,
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
        value: scanned.value.map_err(Error::from),
        diagnostics,
    }
}

// Reads and parses the files in a language on `threads` threads, and keeps the walk order.
fn scan(
    root: &Path,
    files: Vec<RepoFile>,
    threads: usize,
    facts_of: &FactsOf<'_>,
    deadline: Option<&Deadline>,
) -> Noted<Result<RepoFacts, Stopped>> {
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
    let scanned = scan_values(root, files, threads, &file_facts, deadline);
    scanned.map(|scanned| scanned.map(|(files, skipped)| RepoFacts { files, skipped }))
}

type Scanned<T> = Result<Served<Result<T, Skipped>>, Stopped>;

// Each file's value in walk order and the files passed over, or the stop of the call.
type Values<T> = Noted<Result<(Vec<T>, Vec<Skipped>), Stopped>>;

// Each walked file gets one check of `deadline`: in the serial filter when it is not read, and on
// a scan thread when it is.
fn scan_values<T: Send>(
    root: &Path,
    files: Vec<RepoFile>,
    threads: usize,
    value_of: &ValueOf<'_, T>,
    deadline: Option<&Deadline>,
) -> Values<T> {
    let wanted = match in_languages(files, deadline) {
        Ok(wanted) => wanted,
        Err(stopped) => return Noted::new(Err(stopped)),
    };
    let scanned: Vec<Scanned<T>> = map_in_order(&wanted, threads, |(file, language)| {
        checkpoint(deadline)?;
        Ok(scan_one(root, file, *language, value_of))
    });
    collected(scanned)
}

// The files the scan reads. A file it passes over gets its check here.
fn in_languages(
    files: Vec<RepoFile>,
    deadline: Option<&Deadline>,
) -> Result<Vec<(RepoFile, Language)>, Stopped> {
    let mut wanted = Vec::new();
    for file in files {
        match in_language(file) {
            Some(kept) => wanted.push(kept),
            None => checkpoint(deadline)?,
        }
    }
    Ok(wanted)
}

// The values in walk order, up to the first file that the deadline stopped.
fn collected<T>(scanned: Vec<Scanned<T>>) -> Values<T> {
    let mut values = Noted::new((Vec::new(), Vec::new()));
    for served in scanned {
        let Served { value, warning } = match served {
            Ok(served) => served,
            Err(stopped) => return values.map(|_| Err(stopped)),
        };
        values
            .diagnostics
            .extend(warning.map(|failure| failure.to_string()));
        match value {
            Ok(file) => values.value.0.push(file),
            Err(skipped) => values.value.1.push(skipped),
        }
    }
    values.map(Ok)
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
