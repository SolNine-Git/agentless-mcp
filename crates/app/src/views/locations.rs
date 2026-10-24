//! Location strings resolved against one file: `class:`, `function:`, `line:` and `variable:`
//! forms, or the forms that one stable id's qualified name can take.

use facts::{FileSymbols, LocResolution, LocTarget, StableId, Symbol, line_count, resolve_locs};
use syntax::Language;

use super::{load, slice_lines, symbols};
use crate::bounds::within;
use crate::ops::limits::MAX_CONTEXT_LINES;
use crate::{Error, IntArg, Noted, Reading};

/// A request to resolve location strings against one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocsRequest {
    /// The path, as the caller gave it.
    pub path: String,
    /// The location strings, in order.
    pub locs: Vec<String>,
    /// The lines added before and after each resolved span.
    pub context: IntArg,
}

/// A request to find the lines of the symbol that one stable id names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocateRequest {
    /// The id.
    pub id: StableId,
    /// The lines added before and after the symbol.
    pub context: IntArg,
}

/// What a set of location strings resolved to, and the numbered lines of its intervals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocationView {
    /// The path below the repository root.
    pub path: String,
    /// The ids, spans, intervals and misses.
    pub resolution: LocResolution,
    /// The numbered lines of the intervals, or empty when none resolved.
    pub text: String,
}

/// Resolves the location strings of `request` against its file. An unreadable file is a
/// repository-resolution error. The warning of an unreadable cache row comes with either outcome.
#[must_use]
pub fn resolve_locations(
    reading: &Reading<'_>,
    request: &LocsRequest,
) -> Noted<Result<LocationView, Error>> {
    let opened = opened(reading, &request.path, &request.context);
    opened.map(|found| found.and_then(|file| file.view(&request.locs)))
}

/// Resolves the symbol that the id of `request` names, trying each location form its qualified
/// name can take. The first form that resolves wins; when none does, the view lists every miss.
#[must_use]
pub fn locate_symbol(
    reading: &Reading<'_>,
    request: &LocateRequest,
) -> Noted<Result<LocationView, Error>> {
    let opened = opened(reading, request.id.path(), &request.context);
    opened.map(|found| found.and_then(|file| file.symbol(request.id.qualname())))
}

// One readable file, its symbols, and the context lines of every interval.
struct OpenedFile {
    path: String,
    language: Option<Language>,
    text: String,
    symbols: Vec<Symbol>,
    context: u32,
}

fn opened(reading: &Reading<'_>, path: &str, context: &IntArg) -> Noted<Result<OpenedFile, Error>> {
    let context = match within(context, 0, MAX_CONTEXT_LINES, "context") {
        Ok(context) => u32::try_from(context).unwrap_or(u32::MAX),
        Err(error) => return Noted::new(Err(error)),
    };
    let loaded = match load(&reading.repo.root, path) {
        Ok(loaded) => loaded,
        Err(refusal) => return Noted::new(Err(refusal)),
    };
    let text = match loaded.text {
        Ok(text) => text,
        Err(reason) => return Noted::new(Err(kernel::Error::RepoResolution(reason).into())),
    };
    symbols(reading, &loaded.path, loaded.language, &text).map(|symbols| {
        Ok(OpenedFile {
            path: loaded.path,
            language: loaded.language,
            text: text.to_string(),
            symbols,
            context,
        })
    })
}

impl OpenedFile {
    fn view(&self, locs: &[String]) -> Result<LocationView, Error> {
        let target = LocTarget {
            path: &self.path,
            total_lines: line_count(&self.text),
            file: self.language.map(|language| FileSymbols {
                language,
                symbols: &self.symbols,
            }),
        };
        let resolution = resolve_locs(locs, &target, self.context);
        let text = if resolution.intervals.is_empty() {
            String::new()
        } else {
            let rendered = slice_lines(&self.text, &resolution.intervals, &self.symbols)
                .map_err(|outside| kernel::Error::OperationFailed(outside.to_string()))?;
            rendered + "\n"
        };
        Ok(LocationView {
            path: self.path.clone(),
            resolution,
            text,
        })
    }

    // A dotted name is a member, which only the function form reads; a bare name can also be a
    // class of any class-like kind, or a module-level constant.
    fn symbol(&self, qualname: &str) -> Result<LocationView, Error> {
        let others: &[&str] = if qualname.contains('.') {
            &[]
        } else {
            &["class", "variable"]
        };
        let mut view = self.view(&[format!("function: {qualname}")])?;
        let mut misses = view.resolution.unrecognized.clone();
        for form in others {
            if !view.resolution.stable_ids.is_empty() {
                return Ok(view);
            }
            view = self.view(&[format!("{form}: {qualname}")])?;
            misses.extend(view.resolution.unrecognized.iter().cloned());
        }
        if view.resolution.stable_ids.is_empty() {
            view.resolution.unrecognized = misses;
        }
        Ok(view)
    }
}
