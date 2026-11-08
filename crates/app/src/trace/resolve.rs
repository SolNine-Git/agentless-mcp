//! The resolution order of a trace target: a stable id, a `path:line`, a repository-relative
//! path, a qualified name, then a bare name, each name matched as explain matches it. A target
//! that matches nothing is a miss.

use std::collections::BTreeSet;
use std::path::Path;

use facts::{line_count, split_ordinal};
use git::RepoFile;
use graph::{FileFacts, Target, enclosing_symbol, joins_name_graph, parse_target};
use syntax::Language;

use super::{CardKind, MISS_LITERAL_FILES, Miss};
use crate::endpoints::{BindingStatus, Scope, TARGET_WORDING, bind};
use crate::map::focus_resolution;
use crate::ops::limits::MAX_LIMIT;
use crate::search::{SearchError, Selected};
use crate::symbol_graph::{CallGraph, CallScan};
use crate::{Error, Noted, Reading, SymbolCard, symbol_card};

// The paths of `File "<path>", line <n>` frames and of URLs, which the map's focus rule reads.
const FRAME_START: &str = "File \"";
const URL_MARK: &str = "://";

// What one target resolved to, before the operations that fill its card run.
pub(super) enum Resolved {
    Symbol(SymbolCard),
    File(FoundFile),
    Folder(String),
    Candidates(Vec<SymbolCard>),
    Missed(Missed),
}

impl Resolved {
    pub(super) fn kind(&self) -> CardKind {
        match self {
            Resolved::Symbol(_) => CardKind::Symbol,
            Resolved::File(_) => CardKind::File,
            Resolved::Folder(_) => CardKind::Folder,
            Resolved::Candidates(_) => CardKind::Candidates,
            Resolved::Missed(_) => CardKind::Miss,
        }
    }

    // The file of a symbol card or a file card.
    pub(super) fn card_file(&self) -> Option<&str> {
        match self {
            Resolved::Symbol(card) => Some(&card.path),
            Resolved::File(found) => Some(&found.path),
            Resolved::Folder(_) | Resolved::Candidates(_) | Resolved::Missed(_) => None,
        }
    }
}

// A file that the scan read: its declarations in file order, and the stable id of one that joins
// the name graph, which explain reads the file's imports through.
pub(super) struct FoundFile {
    pub(super) path: String,
    pub(super) language: Language,
    pub(super) symbols: Vec<SymbolCard>,
    pub(super) anchor: Option<String>,
    pub(super) no_symbol_at: Option<u32>,
}

// A miss whose answer is known, or a name whose usage files and text are still to be read.
pub(super) enum Missed {
    Ready(Miss),
    Name { name: String, literal: String },
}

// The call's one scan and the name index of its graph resolve every target of the call. The work
// owns what it reads, as a kept graph answers it on the root's graph thread.
pub(super) fn resolved(
    reading: &Reading<'_>,
    scan: &CallScan,
    graph: &CallGraph<'_, '_>,
    walk: &[RepoFile],
    targets: &[String],
) -> Noted<Result<Vec<Resolved>, Error>> {
    let paths: Vec<String> = walk.iter().map(|file| file.path.clone()).collect();
    let (skipped, root) = (scan.skipped.clone(), reading.repo.root.clone());
    let targets = targets.to_vec();
    graph.over(move |graph| {
        let walked = Walked::new(&paths);
        let scope = Scope {
            index: graph.index(),
            skipped: &skipped,
            root: &root,
        };
        let resolver = Resolver {
            scope: &scope,
            walked: &walked,
            files: graph.files(),
        };
        Ok(targets.iter().map(|target| resolver.one(target)).collect())
    })
}

// The walked paths: every file the walk lists, which the scan may not read.
struct Walked<'f> {
    known: BTreeSet<&'f str>,
}

impl<'f> Walked<'f> {
    fn new(paths: &'f [String]) -> Self {
        let known = paths.iter().map(String::as_str).collect();
        Walked { known }
    }

    fn is_file(&self, path: &str) -> bool {
        self.known.contains(path)
    }

    // The root, or a directory that holds a walked file.
    fn is_folder(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        path.is_empty()
            || (self.known.range(prefix.as_str()..).next())
                .is_some_and(|first| first.starts_with(&prefix))
    }

    // The walked files whose path ends with `/path`, in path order, cut at the cap.
    fn endings(&self, path: &str) -> Vec<String> {
        let suffix = format!("/{path}");
        (self.known.iter())
            .filter(|known| known.ends_with(&suffix))
            .take(MISS_LITERAL_FILES)
            .map(|known| (*known).to_owned())
            .collect()
    }
}

// Where a path-shaped target points: a file, a folder, or nothing in the repository.
enum Located {
    File(String),
    Folder(String),
    Unknown(String),
}

struct Resolver<'r, 'a> {
    scope: &'r Scope<'r, 'a>,
    walked: &'r Walked<'r>,
    files: &'a [&'a FileFacts],
}

impl Resolver<'_, '_> {
    fn one(&self, text: &str) -> Resolved {
        if text.trim().is_empty() {
            let (name, literal) = (text.to_owned(), text.to_owned());
            return Resolved::Missed(Missed::Name { name, literal });
        }
        match parse_target(text) {
            Target::Id(..) => self.named(text),
            Target::Line(path, line) => self.at_line(path, line),
            Target::Named(_) => match self.located(text) {
                Some(Located::File(path)) => self.file(path, None),
                Some(Located::Folder(path)) => Resolved::Folder(path),
                Some(Located::Unknown(path)) => self.unknown(path),
                None => self.named(text),
            },
        }
    }

    // A name or a stable id, bound as explain binds it.
    fn named(&self, text: &str) -> Resolved {
        let bound = bind(self.scope, text, MAX_LIMIT, &TARGET_WORDING);
        let cards = || {
            (bound.definitions.iter()).map(|definition| {
                let file = definition.file();
                symbol_card(
                    file.language,
                    &file.path,
                    definition.symbol(),
                    String::new(),
                )
            })
        };
        match bound.binding.status {
            BindingStatus::Bound => Resolved::Symbol(cards().next().expect("one definition")),
            BindingStatus::Ambiguous => Resolved::Candidates(cards().collect()),
            BindingStatus::NotFound => Resolved::Missed(Missed::Name {
                name: usage_name(text),
                literal: text.to_owned(),
            }),
        }
    }

    // The innermost symbol that spans the line, else the file card with the line, else the line
    // past the end.
    fn at_line(&self, path: &str, line: u32) -> Resolved {
        let Some(path) = below_root(self.scope.root, path) else {
            return self.unknown(path.to_owned());
        };
        if !self.walked.is_file(&path) {
            return self.unknown(path);
        }
        let scanned = self.scanned(&path);
        let held = scanned
            .filter(|file| joins_name_graph(file.language))
            .and_then(|file| {
                enclosing_symbol(&file.facts.symbols, line)
                    .map(|symbol| symbol_card(file.language, &file.path, symbol, String::new()))
            });
        if let Some(card) = held {
            return Resolved::Symbol(card);
        }
        let lines = scanned.and_then(|_| read_lines(self.scope.root, &path));
        match lines {
            Some(lines) if line > lines => {
                Resolved::Missed(Missed::Ready(Miss::LinePastEnd { path, line, lines }))
            }
            _ => self.file(path, Some(line)),
        }
    }

    // A path below the root, an absolute path, a URL or a traceback frame; `None` for a target
    // that reads as a name.
    fn located(&self, text: &str) -> Option<Located> {
        let path = relative(text.trim());
        let rooted = match path.strip_prefix('/') {
            Some(_) => below_root(self.scope.root, path),
            None => Some(path.to_owned()),
        };
        let Some(rooted) = rooted else {
            return Some(Located::Unknown(path.to_owned()));
        };
        if self.walked.is_file(&rooted) {
            return Some(Located::File(rooted));
        }
        if self.walked.is_folder(&rooted) {
            return Some(Located::Folder(rooted));
        }
        if text.contains(URL_MARK) || text.trim_start().starts_with(FRAME_START) {
            return Some(self.focused(text, rooted));
        }
        let shaped = rooted.contains('/') || !self.walked.endings(&rooted).is_empty();
        shaped.then_some(Located::Unknown(rooted))
    }

    // A URL or a frame names the one file that the map's focus rule finds for it.
    fn focused(&self, text: &str, rooted: String) -> Located {
        let (paths, _) = focus_resolution(text, &self.walked.known, self.scope.index);
        match paths.as_slice() {
            [only] if self.walked.is_file(only) => Located::File(only.clone()),
            _ => Located::Unknown(rooted),
        }
    }

    fn unknown(&self, path: String) -> Resolved {
        let endings = self.walked.endings(&path);
        Resolved::Missed(Missed::Ready(Miss::UnknownPath { path, endings }))
    }

    // A walked file: its card when the scan read it, else why the scan did not.
    fn file(&self, path: String, no_symbol_at: Option<u32>) -> Resolved {
        if let Some(file) = self.scanned(&path) {
            return Resolved::File(found(file, no_symbol_at));
        }
        let skipped = self.scope.skipped.iter().find(|entry| entry.path == path);
        let miss = match (skipped, Language::from_path(&path)) {
            (Some(entry), _) => Miss::NotParsed {
                path,
                reason: entry.reason.clone(),
            },
            (None, None) => Miss::NoGrammar { path },
            (None, Some(_)) => Miss::NotScanned { path },
        };
        Resolved::Missed(Missed::Ready(miss))
    }

    fn scanned(&self, path: &str) -> Option<&FileFacts> {
        self.files.iter().copied().find(|file| file.path == path)
    }
}

fn found(file: &FileFacts, no_symbol_at: Option<u32>) -> FoundFile {
    let symbols: Vec<SymbolCard> = (file.facts.symbols.iter())
        .map(|symbol| symbol_card(file.language, &file.path, symbol, String::new()))
        .collect();
    let anchor = (joins_name_graph(file.language))
        .then(|| symbols.first().map(|card| card.stable_id.clone()))
        .flatten();
    FoundFile {
        path: file.path.clone(),
        language: file.language,
        symbols,
        anchor,
        no_symbol_at,
    }
}

// The path as the caller wrote it, with no leading `./` and no trailing `/`; `.` is the root.
fn relative(text: &str) -> &str {
    let mut path = text;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    let path = path.trim_end_matches('/');
    if path == "." { "" } else { path }
}

// The path below `root` that `path` names, as a search's path reads it, or `None` for a path
// outside the root.
fn below_root(root: &Path, path: &str) -> Option<String> {
    match Selected::resolve(root, Some(path)) {
        Ok(Selected::File(below) | Selected::Folder(below))
        | Err(SearchError::UnknownPath { path: below }) => Some(below),
        Ok(Selected::Root) => Some(String::new()),
        Err(_) => None,
    }
}

fn read_lines(root: &Path, path: &str) -> Option<u32> {
    let bytes = io::read_bounded(&root.join(path)).ok()?;
    Some(line_count(&String::from_utf8_lossy(&bytes)))
}

// The name that a missed target's usage files are found for: a stable id's qualified name.
fn usage_name(text: &str) -> String {
    match parse_target(text) {
        Target::Id(id, _) => split_ordinal(id.qualname()).0.to_owned(),
        _ => text.to_owned(),
    }
}
