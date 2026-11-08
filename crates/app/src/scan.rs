//! One call's scan: every file the walk lists in a language, read once and turned into facts by
//! the fact source, in walk order, with the files it passed over and why.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use facts::{Facts, Symbol};
use git::RepoFile;
use graph::FileFacts;
use kernel::{Deadline, Stopped, checkpoint};
use store::{FactSource, FileText, RowKey, Served};
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

// The facts of a call's scan, which a server keeps for the next call of the root: the files with
// facts, each shared with the scans that read it, and the row key of each file that came with no
// warning.
#[derive(Default)]
pub(crate) struct KeptFacts {
    pub(crate) files: Arc<Vec<Arc<FileFacts>>>,
    keys: Vec<Option<RowKey>>,
    positions: Arc<HashMap<String, usize>>,
}

impl KeptFacts {
    // The position of the kept facts of the file whose text has `key`.
    fn position(&self, text: &FileText, key: &RowKey) -> Option<usize> {
        let at = *self.positions.get(&text.path)?;
        (self.keys[at].as_ref() == Some(key)).then_some(at)
    }
}

// One file of a scan over kept facts: the position of its kept facts, or its facts with the row
// key to keep them by, which a warning leaves out.
enum Fresh {
    Kept(usize),
    Read(Arc<FileFacts>, Option<RowKey>),
}

// The kept facts after a scan, and the files it passed over, or the stop of the call.
type Rescanned = Noted<Result<(Arc<KeptFacts>, Vec<Skipped>), Stopped>>;

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
        RepoFacts::load_at(
            services,
            &reading.repo.root,
            reading.facts,
            reading.deadline,
        )
    }

    // The scan of the tree at `root`, for a caller with no reading.
    pub(crate) fn load_at(
        services: &Services,
        root: &Path,
        source: &FactSource,
        deadline: Option<&Deadline>,
    ) -> Noted<Result<RepoFacts, Error>> {
        walked(services, root, |files| {
            scan_walked(root, files, source, deadline)
        })
    }
}

// The scan of the walked `files` at `root`, as `RepoFacts::load` scans the files of its own walk.
pub(crate) fn scan_walked(
    root: &Path,
    files: &[RepoFile],
    source: &FactSource,
    deadline: Option<&Deadline>,
) -> Noted<Result<RepoFacts, Stopped>> {
    let facts_of = |text: &FileText| source.facts(text, deadline);
    scan(root, files, threads(), &facts_of, deadline)
}

// Scans the walked `files` at `root`, and takes the facts of each file whose text has its kept
// row key from `kept`. A scan that takes every file from `kept` at its position shares it.
pub(crate) fn rescan(
    root: &Path,
    files: &[RepoFile],
    source: &FactSource,
    deadline: Option<&Deadline>,
    kept: &Arc<KeptFacts>,
) -> Rescanned {
    let facts_of = |text: &FileText| source.facts(text, deadline);
    rescan_with(root, files, &facts_of, deadline, kept)
}

fn rescan_with(
    root: &Path,
    files: &[RepoFile],
    facts_of: &FactsOf<'_>,
    deadline: Option<&Deadline>,
    kept: &Arc<KeptFacts>,
) -> Rescanned {
    let fresh = |text: &FileText| {
        let key = text.row_key();
        if let Some(at) = kept.position(text, &key) {
            let value = Ok(Fresh::Kept(at));
            return Served {
                value,
                warning: None,
            };
        }
        let Served { value, warning } = file_facts(text, facts_of);
        let key = warning.is_none().then_some(key);
        let value = value.map(|facts| Fresh::Read(Arc::new(facts), key));
        Served { value, warning }
    };
    let scanned = scan_values(root, files, threads(), &fresh, deadline);
    scanned.map(|scanned| scanned.map(|(fresh, skipped)| (kept_after(kept, fresh), skipped)))
}

// The kept facts after a scan: `kept` itself when the scan took every file from it in place. A
// kept file's facts are shared, not copied, and the same paths in the same order share positions.
fn kept_after(kept: &Arc<KeptFacts>, fresh: Vec<Fresh>) -> Arc<KeptFacts> {
    let in_place = |(at, file): (usize, &Fresh)| matches!(file, Fresh::Kept(from) if *from == at);
    if fresh.len() == kept.files.len() && fresh.iter().enumerate().all(in_place) {
        return Arc::clone(kept);
    }
    let (files, keys): (Vec<Arc<FileFacts>>, Vec<Option<RowKey>>) = (fresh.into_iter())
        .map(|file| match file {
            Fresh::Kept(at) => (Arc::clone(&kept.files[at]), kept.keys[at]),
            Fresh::Read(facts, key) => (facts, key),
        })
        .unzip();
    let same_paths = files.len() == kept.files.len()
        && (files.iter().zip(kept.files.iter())).all(|(file, held)| file.path == held.path);
    let positions = if same_paths {
        Arc::clone(&kept.positions)
    } else {
        let positions = (files.iter().enumerate()).map(|(at, file)| (file.path.clone(), at));
        Arc::new(positions.collect())
    };
    Arc::new(KeptFacts {
        files: Arc::new(files),
        keys,
        positions,
    })
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
    scan: impl FnOnce(&[RepoFile]) -> Noted<Result<T, Stopped>>,
) -> Noted<Result<T, Error>> {
    let walk = services.git.walk(root, reads);
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
    let scanned = scan(&files);
    diagnostics.extend(scanned.diagnostics);
    Noted {
        value: scanned.value.map_err(Error::from),
        diagnostics,
    }
}

// Reads and parses the files in a language on `threads` threads, and keeps the walk order.
fn scan(
    root: &Path,
    files: &[RepoFile],
    threads: usize,
    facts_of: &FactsOf<'_>,
    deadline: Option<&Deadline>,
) -> Noted<Result<RepoFacts, Stopped>> {
    let file_facts = |text: &FileText| file_facts(text, facts_of);
    let scanned = scan_values(root, files, threads, &file_facts, deadline);
    scanned.map(|scanned| scanned.map(|(files, skipped)| RepoFacts { files, skipped }))
}

fn file_facts(text: &FileText, facts_of: &FactsOf<'_>) -> Served<Result<FileFacts, Unparsable>> {
    let Served { value, warning } = facts_of(text);
    let value = value.map(|facts| FileFacts {
        path: text.path.clone(),
        language: text.language,
        line_count: line_count(&text.text),
        facts,
    });
    Served { value, warning }
}

type Scanned<T> = Result<Served<Result<T, Skipped>>, Stopped>;

// Each file's value in walk order and the files passed over, or the stop of the call.
type Values<T> = Noted<Result<(Vec<T>, Vec<Skipped>), Stopped>>;

// Each walked file gets one check of `deadline`: in the serial filter when it is not read, and on
// a scan thread when it is.
fn scan_values<T: Send>(
    root: &Path,
    files: &[RepoFile],
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
fn in_languages<'f>(
    files: &'f [RepoFile],
    deadline: Option<&Deadline>,
) -> Result<Vec<(&'f RepoFile, Language)>, Stopped> {
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

fn in_language(file: &RepoFile) -> Option<(&RepoFile, Language)> {
    let language = scanned_language(&file.path)?;
    Some((file, language))
}

// The walks' byte bound guards the scan's reads, so it counts what the scan reads: a file in a
// language, within the per-file cap of the read.
pub(crate) fn reads(path: &str, size: u64) -> bool {
    io::within_file_cap(size) && scanned_language(path).is_some()
}

// The project config file is never scanned, and a path whose suffix names no language is not
// scanned.
fn scanned_language(path: &str) -> Option<Language> {
    if path == io::CONFIG_FILENAME {
        return None;
    }
    Language::from_path(path)
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
pub(crate) fn line_count(text: &str) -> u32 {
    u32::try_from(text.split('\n').count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
