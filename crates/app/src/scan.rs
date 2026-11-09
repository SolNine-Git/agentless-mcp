//! One call's scan: every file the walk lists in a language, read once and turned into facts by
//! the fact source, in walk order, with the files it passed over and why.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use facts::{Facts, Symbol};
use git::{Change, CommitId, Git, GitOutcome, RepoFile, Side};
use graph::FileFacts;
use kernel::{Deadline, Stopped, checkpoint};
use prompts::messages::TRACE_REV_UNREAD;
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

// Where a call reads each walked file: the working tree, or a commit.
#[derive(Clone)]
pub(crate) enum Tree {
    Working,
    At(Arc<AtCommit>),
}

// The commit that a call reads, and its name as the caller gave it.
#[derive(Clone, Debug)]
pub(crate) struct Named {
    pub(crate) name: String,
    pub(crate) commit: CommitId,
}

impl Named {
    pub(crate) fn short(&self) -> &str {
        let commit = self.commit.as_str();
        commit.get(..git::SHORT_SHA_LENGTH).unwrap_or(commit)
    }
}

// A commit that a call reads in place of the working tree. A path whose bytes on disk differ from
// the commit's reads as a blob, with its size at the commit; any other path reads from disk.
pub(crate) struct AtCommit {
    named: Named,
    git: Git,
    // `None` reads every path as a blob.
    changed: Option<HashMap<String, Option<u64>>>,
}

impl AtCommit {
    // `changes` lists each path that differs from the commit, and `files` the commit's tree.
    pub(crate) fn new(
        git: Git,
        named: Named,
        changes: Option<&[Change]>,
        files: &[RepoFile],
    ) -> AtCommit {
        let listed: HashMap<&str, u64> = (files.iter())
            .map(|file| (file.path.as_str(), file.size))
            .collect();
        let blobs = changes.map(|differing| {
            let at_commit = differing.iter().filter(|change| change.old != Side::Absent);
            let paths = at_commit.map(|change| {
                let size = listed.get(change.old_path.as_str()).copied();
                (change.old_path.clone(), size)
            });
            paths.collect()
        });
        AtCommit {
            named,
            git,
            changed: blobs,
        }
    }

    fn source(&self, path: &str) -> Source {
        let size = match &self.changed {
            Some(changed) => changed.get(path).copied(),
            None => Some(None),
        };
        size.map_or(Source::Disk, |size| Source::Blob { size })
    }

    // The bytes of `path` at the commit, or why they were not read.
    fn blob(&self, root: &Path, path: &str, size: Option<u64>) -> Result<Vec<u8>, String> {
        let too_large = |size| io::Skipped::TooLarge { size }.to_string();
        if let Some(size) = size.filter(|size| !io::within_file_cap(*size)) {
            return Err(too_large(size));
        }
        let note = match self.git.blob(root, self.named.commit.as_str(), path) {
            GitOutcome::Printed(bytes) => return Ok(bytes),
            GitOutcome::Capped(bytes) => return Err(too_large(size.unwrap_or(bytes.len() as u64))),
            GitOutcome::Failed(failure) => failure.to_string(),
            GitOutcome::Unfinished(unfinished) => unfinished.to_string(),
        };
        Err(TRACE_REV_UNREAD.render((path, &self.named.name, &note)))
    }
}

// Where the bytes of one path at a commit come from: the disk, or a blob of the listed size.
enum Source {
    Disk,
    Blob { size: Option<u64> },
}

impl Tree {
    // The bytes of the walked file at `path`, or the scan's reason for passing over it.
    pub(crate) fn read(&self, root: &Path, path: &str) -> Result<Vec<u8>, String> {
        if let Tree::At(at) = self
            && let Source::Blob { size } = at.source(path)
        {
            return at.blob(root, path, size);
        }
        io::read_bounded(&root.join(path)).map_err(|skipped| skipped.to_string())
    }

    // The commit that the call reads, or `None` for the working tree.
    pub(crate) fn commit(&self) -> Option<&Named> {
        match self {
            Tree::Working => None,
            Tree::At(at) => Some(&at.named),
        }
    }
}

// The scan of one repository: the files with facts, and the files passed over, in walk order.
#[derive(Debug, Default)]
pub(crate) struct RepoFacts {
    pub(crate) files: Vec<FileFacts>,
    pub(crate) skipped: Vec<Skipped>,
}

// The facts of a call's scan, which a server keeps for the next call of the root: the files with
// facts, and the row key of each file that came with no warning.
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
            scan_walked(root, (files, &Tree::Working), source, deadline)
        })
    }
}

// The scan of the walked `files` at `root`, read from `tree`, as `RepoFacts::load` scans the files
// of its own walk.
pub(crate) fn scan_walked(
    root: &Path,
    (files, tree): (&[RepoFile], &Tree),
    source: &FactSource,
    deadline: Option<&Deadline>,
) -> Noted<Result<RepoFacts, Stopped>> {
    let facts_of = |text: &FileText| source.facts(text, deadline);
    let at = Walked { root, files, tree };
    scan(at, threads(), &facts_of, deadline)
}

// The walked files of one scan, the root they are below, and the tree that their bytes come from.
#[derive(Clone, Copy)]
struct Walked<'w> {
    root: &'w Path,
    files: &'w [RepoFile],
    tree: &'w Tree,
}

// Scans the walked `files` at `root`, and takes the facts of each file whose text has its kept
// row key from `kept`. A scan that takes every file from `kept` at its position shares it.
pub(crate) fn rescan(
    root: &Path,
    (files, tree): (&[RepoFile], &Tree),
    source: &FactSource,
    deadline: Option<&Deadline>,
    kept: &Arc<KeptFacts>,
) -> Rescanned {
    let facts_of = |text: &FileText| source.facts(text, deadline);
    let at = Walked { root, files, tree };
    rescan_with(at, &facts_of, deadline, kept)
}

fn rescan_with(
    at: Walked<'_>,
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
    let scanned = scan_values(at, threads(), &fresh, deadline);
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
            let at = Walked {
                root,
                files,
                tree: &Tree::Working,
            };
            let scanned = scan_values(at, threads(), &symbols_of, reading.deadline);
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
    walked_files(services, root).then_ok(|files| {
        let scanned = scan(&files);
        scanned.map(|value| value.map_err(Error::from))
    })
}

// The walk that tree, index and every scan list, with its warnings.
pub(crate) fn walked_files(
    services: &Services,
    root: &Path,
) -> Noted<Result<Vec<RepoFile>, Error>> {
    let walk = services.git.walk(root, reads);
    let diagnostics = walk.warnings.iter().map(ToString::to_string).collect();
    let value = walk
        .files
        .map_err(|refusal| kernel::Error::from(refusal).into());
    Noted { value, diagnostics }
}

// The facts of `text` parsed as `language`, or `None` when the grammar refuses it.
pub(crate) fn parsed_file(path: &str, language: Language, text: &str) -> Option<FileFacts> {
    let file = FileText {
        path: path.to_owned(),
        language,
        text: Arc::from(text),
    };
    let analysis = store::analyze(&file, false).ok()?;
    Some(FileFacts {
        path: file.path,
        language,
        line_count: line_count(text),
        facts: analysis.facts,
    })
}

// Reads and parses the files in a language on `threads` threads, and keeps the walk order.
fn scan(
    at: Walked<'_>,
    threads: usize,
    facts_of: &FactsOf<'_>,
    deadline: Option<&Deadline>,
) -> Noted<Result<RepoFacts, Stopped>> {
    let file_facts = |text: &FileText| file_facts(text, facts_of);
    let scanned = scan_values(at, threads, &file_facts, deadline);
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
    at: Walked<'_>,
    threads: usize,
    value_of: &ValueOf<'_, T>,
    deadline: Option<&Deadline>,
) -> Values<T> {
    let wanted = match in_languages(at.files, deadline) {
        Ok(wanted) => wanted,
        Err(stopped) => return Noted::new(Err(stopped)),
    };
    let scanned: Vec<Scanned<T>> = map_in_order(&wanted, threads, |(file, language)| {
        checkpoint(deadline)?;
        Ok(scan_one(at, file, *language, value_of))
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
    at: Walked<'_>,
    file: &RepoFile,
    language: Language,
    value_of: &ValueOf<'_, T>,
) -> Served<Result<T, Skipped>> {
    let bytes = match at.tree.read(at.root, &file.path) {
        Ok(bytes) => bytes,
        Err(reason) => {
            return Served {
                value: Err(skip(&file.path, &reason)),
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
