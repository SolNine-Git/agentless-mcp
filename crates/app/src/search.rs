//! The text search over the repository walk: the lines that a regular expression matches, grouped
//! by file and by the symbol that holds them, with lock, vendored and minified files counted
//! apart; or the files that a glob lists, with their line and symbol counts.

mod files;
mod lines;
mod noise;
mod pattern;
mod shape;

use std::borrow::Cow;
use std::path::Path;

use git::RepoFile;
use kernel::checkpoint;
use regex::Regex;

use crate::bounds::within;
use crate::{Error, IntArg, Noted, Reading, Services, Skipped};
use files::in_scope;
use lines::hits_in;
use shape::{Gathered, Shaped, owner_blocks, shaped};

pub(crate) use files::file_row;
pub use files::{FileRow, Measure, is_source_path};
pub(crate) use files::{Selected, walked_files};
pub(crate) use lines::FileHits;
pub use noise::{
    MINIFIED_KEEP_CHARS, MINIFIED_LONGEST_LINE_BYTES, MINIFIED_MEAN_LINE_BYTES, NoiseClass,
};
pub use pattern::{PatternError, PatternFault};

/// The `limit` of a search that does not set one: match lines, or file rows of a listing.
pub const DEFAULT_SEARCH_LIMIT: usize = 100;
/// The largest `limit` of a search.
pub const MAX_SEARCH_LIMIT: usize = 1_000;
/// The most context lines before and after each match.
pub const MAX_SEARCH_CONTEXT: usize = 10;
/// How many places a shown match line's text needs before the answer prints it once.
pub const REPEAT_PLACES: usize = 3;

/// A search request: the pattern, the glob, the file or folder, and the bounds of the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRequest {
    /// The regular expression that each line is matched against, or `None` for a file listing.
    pub pattern: Option<String>,
    /// The glob on repository-relative paths that a file must match, or `None` for every file.
    pub files: Option<String>,
    /// The repository-relative file or folder to search, or `None` for the root.
    pub path: Option<String>,
    /// Whether the pattern ignores letter case.
    pub ignore_case: bool,
    /// The lines shown before and after each match.
    pub context: IntArg,
    /// The most match lines, or the most file rows of a listing.
    pub limit: IntArg,
}

/// Why a search gives no answer.
#[derive(Debug)]
pub enum SearchError {
    /// The pattern is not a valid regular expression.
    Pattern(PatternError),
    /// The request has neither a pattern nor a glob.
    NothingAsked,
    /// The glob expands to more globs than [`text::MAX_BRACE_GLOBS`].
    WideGlob {
        /// The glob as the caller wrote it.
        glob: String,
    },
    /// The path names no file or folder in the repository.
    UnknownPath {
        /// The path below the root.
        path: String,
    },
    /// A bound, the path's containment, the walk or the call's deadline refused the search.
    Refused(Error),
}

/// What a search found: the match lines, or the file rows of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Searched {
    /// The answer to a request with a pattern.
    Matches(Matches),
    /// The answer to a request with a glob and no pattern.
    Listing(Listing),
}

/// The match lines of a search, the counts behind them, and what the answer counts apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matches {
    /// The match lines in every file searched, noise files included.
    pub matches: usize,
    /// The files with a match, noise files included.
    pub files: usize,
    /// The files read and searched.
    pub searched: usize,
    /// The shown match lines by file: source files first, each part in path order.
    pub groups: Vec<FileMatches>,
    /// The shown match lines whose text repeats in [`REPEAT_PLACES`] or more places, printed once.
    pub repeats: Vec<Repeat>,
    /// The match lines that the limit kept, in the groups and the repeats.
    pub shown: usize,
    /// The files whose match lines the limit left out, with how many it left out of each.
    pub rest: Vec<FileCount>,
    /// The noise files, as one count per class.
    pub noise: Vec<Noise>,
    /// The files that the search could not read.
    pub skipped: Vec<Skipped>,
}

/// The shown lines of one file, in blocks by the symbol that holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    /// The path below the repository root.
    pub path: String,
    /// The blocks, in line order.
    pub blocks: Vec<Block>,
}

/// Consecutive shown lines that one symbol holds, or that no symbol holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The innermost symbol that spans the lines, or `None` outside every symbol.
    pub owner: Option<Owner>,
    /// The lines, in line order.
    pub lines: Vec<Line>,
}

/// The symbol that holds a match line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// The qualified name.
    pub qualname: String,
    /// The stable id, which trace reads.
    pub stable_id: String,
    /// The first line of the span.
    pub start_line: u32,
    /// The last line of the span.
    pub end_line: u32,
}

/// One shown line: a match line or a context line, with its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The 1-based line number.
    pub number: u32,
    /// The text, without the line break, cut only in a minified file.
    pub text: String,
    /// Whether the pattern matches the line, as against a context line.
    pub matched: bool,
    /// How many characters a minified file's cut left out, or `None` for a whole line.
    pub cut: Option<usize>,
}

/// One match line text and every place that the answer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    /// The line's text.
    pub text: String,
    /// The places, in the order the answer reaches them.
    pub places: Vec<Place>,
}

/// One line of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The path below the repository root.
    pub path: String,
    /// The 1-based line number.
    pub line: u32,
}

/// A file and a count of its match lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCount {
    /// The path below the repository root.
    pub path: String,
    /// The match lines.
    pub count: usize,
}

/// The match lines of the files of one noise class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noise {
    /// The class.
    pub class: NoiseClass,
    /// The match lines in these files.
    pub matches: usize,
    /// Each file of the class with a match, in path order, and its match lines.
    pub files: Vec<FileCount>,
}

/// The files that a glob lists, cut at the limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The files that the glob and the path select.
    pub files: usize,
    /// The first rows, in path order.
    pub rows: Vec<FileRow>,
}

/// Searches the repository that `reading` names. The walk's warnings and the warnings of
/// unreadable cache rows come with either outcome.
#[must_use]
pub fn search(
    services: &Services,
    reading: &Reading<'_>,
    request: &SearchRequest,
) -> Noted<Result<Searched, SearchError>> {
    let asked = match Asked::check(&reading.repo.root, request) {
        Ok(asked) => asked,
        Err(error) => return Noted::new(Err(error)),
    };
    let walked = walked_files(services, &reading.repo.root);
    let mut diagnostics = walked.diagnostics;
    let answered = walked.value.map_err(SearchError::Refused).map(|files| {
        let scoped = in_scope(&files, &asked.selected, asked.globs.as_deref());
        asked.answer(reading, &scoped)
    });
    let value = match answered {
        Ok(answer) => {
            diagnostics.extend(answer.diagnostics);
            answer.value
        }
        Err(error) => Err(error),
    };
    Noted { value, diagnostics }
}

// A request after its checks: the compiled pattern, the expanded globs and the selected path.
struct Asked {
    regex: Option<Regex>,
    globs: Option<Vec<String>>,
    selected: Selected,
    context: usize,
    limit: usize,
}

impl Asked {
    // The bounds first, with the messages that every operation shares, then the request's own
    // checks.
    fn check(root: &Path, request: &SearchRequest) -> Result<Asked, SearchError> {
        let limit = within(&request.limit, 1, MAX_SEARCH_LIMIT, "limit");
        let limit = limit.map_err(SearchError::Refused)?;
        let context = within(&request.context, 0, MAX_SEARCH_CONTEXT, "context");
        let context = context.map_err(SearchError::Refused)?;
        if request.pattern.is_none() && request.files.is_none() {
            return Err(SearchError::NothingAsked);
        }
        let regex = (request.pattern.as_deref())
            .map(|pattern| pattern::compiled(pattern, request.ignore_case))
            .transpose()
            .map_err(SearchError::Pattern)?;
        let globs = request.files.as_deref().map(expanded).transpose()?;
        let selected = Selected::resolve(root, request.path.as_deref())?;
        Ok(Asked {
            regex,
            globs,
            selected,
            context,
            limit,
        })
    }

    fn answer(
        &self,
        reading: &Reading<'_>,
        files: &[&RepoFile],
    ) -> Noted<Result<Searched, SearchError>> {
        let Some(regex) = &self.regex else {
            let mut rows = Noted::new(Vec::new());
            for file in files.iter().take(self.limit) {
                let row = file_row(reading, &file.path);
                rows.diagnostics.extend(row.diagnostics);
                rows.value.push(row.value);
            }
            return rows.map(|rows| {
                Ok(Searched::Listing(Listing {
                    files: files.len(),
                    rows,
                }))
            });
        };
        let gathered = Gathered::new(self.selected.file(), self.limit, self.context);
        let read = match read_hits(reading, files, regex, gathered) {
            Ok(read) => read,
            Err(error) => return Noted::new(Err(SearchError::Refused(error))),
        };
        owned(reading, shaped(read.gathered)).map(|shaped| {
            Ok(Searched::Matches(Matches {
                searched: read.searched,
                skipped: read.skipped,
                ..shaped
            }))
        })
    }
}

fn expanded(glob: &str) -> Result<Vec<String>, SearchError> {
    text::expand_braces(glob).ok_or_else(|| SearchError::WideGlob {
        glob: glob.to_owned(),
    })
}

// The files read, the ones that could not be read, and the hits of each file with a match.
struct Read<'n> {
    gathered: Gathered<'n>,
    searched: usize,
    skipped: Vec<Skipped>,
}

impl Read<'_> {
    fn search(&mut self, path: &str, bytes: &[u8], regex: &Regex) {
        let Some(text) = searched_text(bytes) else {
            return;
        };
        self.searched += 1;
        if let Some(hits) = hits_in(path, &text, regex, self.gathered.context()) {
            self.gathered.add(hits, || files::parsed_text(path, bytes));
        }
    }
}

// Each file is read once, after one check of the call's deadline.
fn read_hits<'n>(
    reading: &Reading<'_>,
    files: &[&RepoFile],
    regex: &Regex,
    gathered: Gathered<'n>,
) -> Result<Read<'n>, Error> {
    let mut read = Read {
        gathered,
        searched: 0,
        skipped: Vec::new(),
    };
    for file in files {
        checkpoint(reading.deadline)?;
        match io::read_bounded(&reading.repo.root.join(&file.path)) {
            Ok(bytes) => read.search(&file.path, &bytes, regex),
            Err(skipped) => read.skipped.push(Skipped {
                path: file.path.clone(),
                reason: skipped.to_string(),
            }),
        }
    }
    Ok(read)
}

// The shaped answer with the owner of each shown line, from the symbols of the text that the
// search read.
fn owned(reading: &Reading<'_>, shaped: Shaped) -> Noted<Matches> {
    let mut diagnostics = Vec::new();
    let groups = (shaped.groups.into_iter())
        .map(|(path, lines)| {
            let symbols = (shaped.texts.get(&path))
                .map_or_else(|| Noted::new(None), |text| files::symbols_of(reading, text));
            diagnostics.extend(symbols.diagnostics);
            let blocks = owner_blocks(&path, lines, symbols.value.as_ref());
            FileMatches { path, blocks }
        })
        .collect();
    Noted {
        value: Matches {
            groups,
            ..shaped.matches
        },
        diagnostics,
    }
}

// The text that a search reads of a file's bytes, or `None` for a binary file, which ripgrep
// skips.
pub(crate) fn searched_text(bytes: &[u8]) -> Option<Cow<'_, str>> {
    (!git::looks_binary(bytes)).then(|| String::from_utf8_lossy(bytes))
}

// The text of a trace miss's target, matched as a search for its escaped text matches it: a line
// matches when it holds the text.
pub(crate) struct Literal {
    text: String,
    regex: Regex,
}

impl Literal {
    pub(crate) fn new(literal: &str) -> Result<Literal, Error> {
        match Regex::new(&regex::escape(literal)) {
            Ok(regex) => Ok(Literal {
                text: literal.to_owned(),
                regex,
            }),
            Err(error) => {
                let message = format!("{literal}: {error}");
                Err(kernel::Error::OperationFailed(message).into())
            }
        }
    }

    // The hits in one file's searched text. A line holds the text only where the whole text does,
    // so a text that does not hold it is not split into lines.
    pub(crate) fn hits(&self, path: &str, text: &str) -> Option<FileHits> {
        if !text.contains(&self.text) {
            return None;
        }
        hits_in(path, text, &self.regex, 0)
    }
}

// The first match line of each file with a hit, in the answer's file order, cut at `cap`, and how
// many files hold a hit.
pub(crate) fn literal_places(hits: Vec<FileHits>, cap: usize) -> (Vec<Place>, usize) {
    let total = hits.len();
    (shape::first_places(hits, cap), total)
}

#[cfg(test)]
mod tests;
