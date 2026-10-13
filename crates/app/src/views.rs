//! The file views: each named file as signatures with elided bodies, and numbered line slices with
//! scope headers.

mod locations;

use std::path::Path;
use std::sync::Arc;

use facts::{SkeletonOptions, Symbol, line_count, skeletonize, slice_lines};
use prompts::messages::SLICE_RANGE_BEYOND_FILE;
use store::{FileText, Served};
use syntax::Language;

use crate::bounds::within;
use crate::ops::limits::MAX_CONTEXT_LINES;
use crate::{Error, IntArg, Noted, Reading};

pub use locations::{LocateRequest, LocationView, LocsRequest, locate_symbol, resolve_locations};

/// One file at some zoom level, or why it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileView {
    /// The path below the repository root, or the argument as given when it was refused.
    pub path: String,
    /// The language name, or empty when no grammar reads the file.
    pub language: String,
    /// The rendered text.
    pub text: String,
    /// Why the file has no text, or empty.
    pub error: String,
    /// True when the path lies outside the repository.
    pub refused: bool,
}

/// A skeleton request: the files, and what the skeletons keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkeletonRequest {
    /// The paths, as the caller gave them.
    pub paths: Vec<String>,
    /// True keeps docstrings, false drops them, `None` defers to the project config.
    pub docstrings: Option<bool>,
    /// True prefixes each line with its number.
    pub numbered: bool,
}

/// A slice request: the file, its line ranges, and the context lines around each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceRequest {
    /// The path, as the caller gave it.
    pub path: String,
    /// The 1-based inclusive ranges; none asks for the whole file.
    pub ranges: Vec<(usize, usize)>,
    /// The lines added before and after each range.
    pub context: IntArg,
}

/// Renders each file of `request` as signatures with elided bodies, one view per path in order.
/// A refused or unreadable path is a view with its reason.
#[must_use]
pub fn skeleton(reading: &Reading<'_>, request: &SkeletonRequest) -> Vec<FileView> {
    let docstrings = request
        .docstrings
        .or(reading.repo.config.docstrings)
        .unwrap_or(false);
    let options = SkeletonOptions {
        docstrings,
        number_lines: request.numbered,
    };
    let root = &reading.repo.root;
    request
        .paths
        .iter()
        .map(|raw| skeleton_view(root, raw, options))
        .collect()
}

fn skeleton_view(root: &Path, raw: &str, options: SkeletonOptions) -> FileView {
    let loaded = match load(root, raw) {
        Ok(loaded) => loaded,
        Err(refusal) => {
            return FileView {
                refused: true,
                ..failed(raw, None, refusal.to_string())
            };
        }
    };
    let (text, language) = match (loaded.text, loaded.language) {
        (Ok(text), Some(language)) => (text, language),
        (Ok(_), None) => {
            let reason = format!("{}: no grammar for this file type", loaded.path);
            return failed(&loaded.path, None, reason);
        }
        (Err(reason), language) => return failed(&loaded.path, language, reason),
    };
    match syntax::parse(language, text) {
        Ok(parsed) => FileView {
            text: skeletonize(&parsed, options),
            ..failed(&loaded.path, Some(language), String::new())
        },
        Err(unparsable) => failed(&loaded.path, Some(language), unparsable.to_string()),
    }
}

fn failed(path: &str, language: Option<Language>, error: String) -> FileView {
    FileView {
        path: path.to_owned(),
        language: language.map_or_else(String::new, |language| language.name().to_owned()),
        text: String::new(),
        error,
        refused: false,
    }
}

/// Renders the lines of `request` with the headers of the scopes around them. A range past the
/// end of the file gets a line that says so. The warning of an unreadable cache row comes with
/// either outcome.
#[must_use]
pub fn slice(reading: &Reading<'_>, request: &SliceRequest) -> Noted<Result<FileView, Error>> {
    let context = match within(&request.context, 0, MAX_CONTEXT_LINES, "context") {
        Ok(context) => context,
        Err(error) => return Noted::new(Err(error)),
    };
    let loaded = match load(&reading.repo.root, &request.path) {
        Ok(loaded) => loaded,
        Err(refusal) => return Noted::new(Err(refusal)),
    };
    let text = match loaded.text {
        Ok(text) => text,
        Err(reason) => return Noted::new(Ok(failed(&loaded.path, loaded.language, reason))),
    };
    let symbols = symbols(reading, &loaded.path, loaded.language, &text);
    symbols.map(|symbols| {
        let file = Sliced {
            path: &loaded.path,
            language: loaded.language,
            text: &text,
            symbols: &symbols,
        };
        file.view(&request.ranges, context)
    })
}

// One readable file and the symbols whose headers its slices repeat.
struct Sliced<'a> {
    path: &'a str,
    language: Option<Language>,
    text: &'a str,
    symbols: &'a [Symbol],
}

impl Sliced<'_> {
    // The satisfiable ranges widen by the context and render; each other range gets a report.
    fn view(&self, ranges: &[(usize, usize)], context: usize) -> Result<FileView, Error> {
        let total = usize::try_from(line_count(self.text)).unwrap_or(usize::MAX);
        let (fits, beyond): (Vec<_>, Vec<_>) =
            ranges.iter().partition(|&&(start, _)| start <= total);
        let reports: Vec<String> = beyond
            .iter()
            .map(|&&(start, end)| SLICE_RANGE_BEYOND_FILE.render((self.path, start, end, total)))
            .collect();
        let mut view = failed(self.path, self.language, String::new());
        if !ranges.is_empty() && fits.is_empty() {
            view.text = reports.join("\n") + "\n";
            return Ok(view);
        }
        let widened: Vec<(u32, u32)> = fits
            .iter()
            .map(|&&(start, end)| widen(start, end, context, total))
            .collect();
        let rendered = slice_lines(self.text, &widened, self.symbols)
            .map_err(|outside| kernel::Error::OperationFailed(outside.to_string()))?;
        view.text = [reports, vec![rendered]].concat().join("\n") + "\n";
        Ok(view)
    }
}

fn widen(start: usize, end: usize, context: usize, total: usize) -> (u32, u32) {
    let first = start.saturating_sub(context).max(1);
    let last = end.saturating_add(context).min(total);
    let narrow = |line: usize| u32::try_from(line).unwrap_or(u32::MAX);
    (narrow(first), narrow(last))
}

// One repository file as a view reads it: its path below the root, its language when a grammar
// reads it, and its text or why it has none.
struct Loaded {
    path: String,
    language: Option<Language>,
    text: Result<Arc<str>, String>,
}

// The refusal of a path outside the repository is the error; an unreadable file is a reason.
fn load(root: &Path, raw: &str) -> Result<Loaded, Error> {
    let absolute = io::contained_path(root, Path::new(raw))
        .map_err(|refusal| kernel::Error::SecurityRefusal(refusal.to_string()))?;
    let below = absolute.strip_prefix(root).unwrap_or(&absolute);
    let path = match below.to_string_lossy().into_owned() {
        empty if empty.is_empty() => ".".to_owned(),
        path => path,
    };
    let language = Language::from_path(&absolute.to_string_lossy());
    let text = match io::read_bounded(&absolute) {
        Ok(bytes) => Ok(decoded(&path, language, &bytes)),
        Err(skipped) => Err(format!("{path}: {skipped}")),
    };
    Ok(Loaded {
        path,
        language,
        text,
    })
}

// The text of a file in a language decodes as the fact source decodes it; a file in no language
// keeps the same replacement rule.
fn decoded(path: &str, language: Option<Language>, bytes: &[u8]) -> Arc<str> {
    match language {
        Some(language) => FileText::decode(path.to_owned(), language, bytes).text,
        None => Arc::from(String::from_utf8_lossy(bytes).as_ref()),
    }
}

// The symbols whose headers a slice repeats: none for a file in no language or one that does not
// parse. The warning of an unreadable cache row comes with them.
fn symbols(
    reading: &Reading<'_>,
    path: &str,
    language: Option<Language>,
    text: &Arc<str>,
) -> Noted<Vec<Symbol>> {
    let Some(language) = language else {
        return Noted::new(Vec::new());
    };
    let file = FileText {
        path: path.to_owned(),
        language,
        text: Arc::clone(text),
    };
    let Served { value, warning } = reading.facts.symbols(&file);
    Noted {
        value: value.unwrap_or_default(),
        diagnostics: warning.iter().map(ToString::to_string).collect(),
    }
}

#[cfg(test)]
mod tests;
