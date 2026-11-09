//! The files that a search reads: the walk below the selected file or folder that the globs match,
//! and the row of one file with its line count and symbol count.

use std::path::Path;

use facts::{Symbol, line_count};
use git::RepoFile;
use store::{FileText, Served};
use syntax::Language;

use super::SearchError;
use crate::views::contained;
use crate::{Noted, Reading};

// The grammars that read data or queries, not code: their files print after the source files.
const NOT_SOURCE: [Language; 5] = [
    Language::Json,
    Language::Yaml,
    Language::Toml,
    Language::Hcl,
    Language::Sql,
];

/// One file of a listing: its path, and its line and symbol counts or why it was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    /// The path below the repository root.
    pub path: String,
    /// The counts, or why the file has none.
    pub measure: Measure,
}

/// A file's line count and symbol count, or why the file was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Measure {
    /// The file was read.
    Counted {
        /// The lines of the file.
        lines: u32,
        /// The symbols that the tag cache or a parse gives, or `None` when no grammar reads the
        /// file or the grammar refused it.
        symbols: Option<usize>,
    },
    /// The file was not read, and the reason, such as the per-file cap.
    Unread(String),
}

/// Returns true when a code grammar reads `path`: JSON, YAML, TOML, HCL and SQL do not count.
#[must_use]
pub fn is_source_path(path: &str) -> bool {
    Language::from_path(path).is_some_and(|language| !NOT_SOURCE.contains(&language))
}

// The part of the repository that a search reads: the root, one file, or one folder.
pub(crate) enum Selected {
    Root,
    File(String),
    Folder(String),
}

impl Selected {
    // A path outside the root gets the containment refusal that every view gives.
    pub(crate) fn resolve(root: &Path, path: Option<&str>) -> Result<Selected, SearchError> {
        let Some(path) = path else {
            return Ok(Selected::Root);
        };
        let (absolute, below) =
            contained(root, path).map_err(|error| SearchError::Refused(error.into()))?;
        if below.is_empty() {
            Ok(Selected::Root)
        } else if absolute.is_dir() {
            Ok(Selected::Folder(below))
        } else if absolute.is_file() {
            Ok(Selected::File(below))
        } else {
            Err(SearchError::UnknownPath { path: below })
        }
    }

    // The file that the path names, which the answer shows in full whatever its class.
    pub(super) fn file(&self) -> Option<&str> {
        match self {
            Selected::File(file) => Some(file),
            Selected::Root | Selected::Folder(_) => None,
        }
    }

    fn holds(&self, path: &str) -> bool {
        match self {
            Selected::Root => true,
            Selected::File(file) => path == file,
            Selected::Folder(folder) => path
                .strip_prefix(folder.as_str())
                .is_some_and(|rest| rest.starts_with('/')),
        }
    }
}

// The walked files that the selection holds and one of `globs` matches, in walk order.
pub(super) fn in_scope<'f>(
    files: &'f [RepoFile],
    selected: &Selected,
    globs: Option<&[String]>,
) -> Vec<&'f RepoFile> {
    let globbed = |path: &str| globs.is_none_or(|globs| text::in_boundary(path, globs));
    (files.iter())
        .filter(|file| selected.holds(&file.path) && globbed(&file.path))
        .collect()
}

// The row of the file at `path`: its lines, and its symbols from the cache or a parse.
pub(crate) fn file_row(reading: &Reading<'_>, path: &str) -> Noted<FileRow> {
    let bytes = io::read_bounded(&reading.repo.root.join(path));
    file_row_from(reading, path, bytes.map_err(|skipped| skipped.to_string()))
}

// The row of the file at `path` from its bytes, or from why they were not read.
pub(crate) fn file_row_from(
    reading: &Reading<'_>,
    path: &str,
    bytes: Result<Vec<u8>, String>,
) -> Noted<FileRow> {
    let row = |measure| FileRow {
        path: path.to_owned(),
        measure,
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(reason) => return Noted::new(row(Measure::Unread(reason))),
    };
    let symbols = parsed_text(path, &bytes)
        .map_or_else(|| Noted::new(None), |text| symbols_of(reading, &text));
    let lines = line_count(&String::from_utf8_lossy(&bytes));
    symbols.map(|symbols| {
        row(Measure::Counted {
            lines,
            symbols: symbols.map(|symbols| symbols.len()),
        })
    })
}

// The text of the file at `path` that the symbol reader takes, or `None` when no grammar reads it.
pub(super) fn parsed_text(path: &str, bytes: &[u8]) -> Option<FileText> {
    Language::from_path(path).map(|language| FileText::decode(path.to_owned(), language, bytes))
}

// The symbols of `text`, or `None` when its grammar refused it.
pub(super) fn symbols_of(reading: &Reading<'_>, text: &FileText) -> Noted<Option<Vec<Symbol>>> {
    let Served { value, warning } = reading.facts.symbols(text, reading.deadline);
    Noted {
        value: value.ok(),
        diagnostics: warning.iter().map(ToString::to_string).collect(),
    }
}
