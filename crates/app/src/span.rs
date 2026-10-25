//! One stable id resolved to its symbol and the lines that the symbol spans in the working tree.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use facts::{OUTLINE_CAP, StableId, Symbol, is_data_language, line_count};
use prompts::messages::SPAN_DATA_KEY_UNLISTED;
use store::{FileText, Served};
use syntax::Language;

use crate::{Noted, Reading};

// One symbol, the file that declares it, and its lines in the working tree.
#[derive(Clone, Debug)]
pub(crate) struct SymbolSpan {
    pub(crate) symbol: Symbol,
    pub(crate) language: Language,
    pub(crate) path: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) source: Arc<str>,
}

impl SymbolSpan {
    pub(crate) fn stable_id(&self) -> StableId {
        StableId::of(self.language, &self.path, &self.symbol)
    }
}

// Why an id did not resolve: a reason the answer reports, or a path outside the root.
#[derive(Clone, Debug)]
pub(crate) enum Missed {
    Reason(String),
    Refused(io::Refusal),
}

pub(crate) struct ParsedFile {
    language: Language,
    source: Arc<str>,
    symbols: Vec<Symbol>,
}

// One call's parsed files, by the path each id spells, with the warnings of each parse. A later
// call reads the working tree again, so an edit between calls shows.
pub(crate) type ParsedFiles = HashMap<String, Noted<Result<ParsedFile, Missed>>>;

// The cache's row warning comes with either outcome.
pub(crate) fn resolve_symbol_span(
    reading: &Reading<'_>,
    raw: &str,
) -> Noted<Result<SymbolSpan, Missed>> {
    resolve_symbol_span_in(reading, raw, &mut ParsedFiles::new())
}

// The key that a data file's stable id names, or `None` when `raw` is no such id or the file
// does not hold the key. Data keys stay out of the name graph, so only the file resolves them.
pub(crate) fn data_key_span(reading: &Reading<'_>, raw: &str) -> Option<Noted<SymbolSpan>> {
    let parsed: StableId = raw.parse().ok()?;
    let language = Language::from_path(parsed.path())?;
    if !is_data_language(language) {
        return None;
    }
    let found = resolve_symbol_span(reading, raw);
    let span = found.value.ok()?;
    Some(Noted {
        value: span,
        diagnostics: found.diagnostics,
    })
}

// As `resolve_symbol_span`, with a path parsed once per `files`. Each id that names the path
// gets the parse's warnings.
pub(crate) fn resolve_symbol_span_in(
    reading: &Reading<'_>,
    raw: &str,
    files: &mut ParsedFiles,
) -> Noted<Result<SymbolSpan, Missed>> {
    let parsed: StableId = match raw.parse() {
        Ok(parsed) => parsed,
        Err(error) => return Noted::new(Err(Missed::Reason(error.to_string()))),
    };
    let path = parsed.path();
    let file = (files.entry(path.to_owned())).or_insert_with(|| parse_file(reading, path));
    Noted {
        value: (file.value.as_ref())
            .map_err(Missed::clone)
            .and_then(|file| span(&parsed, file)),
        diagnostics: file.diagnostics.clone(),
    }
}

fn span(parsed: &StableId, file: &ParsedFile) -> Result<SymbolSpan, Missed> {
    let (path, qualname) = (parsed.path(), parsed.qualname());
    let ParsedFile {
        language,
        source,
        symbols,
    } = file;
    let language = *language;
    let Some(symbol) = symbols
        .iter()
        .find(|symbol| symbol.id_qualname() == qualname)
        .cloned()
    else {
        return Err(Missed::Reason(undefined(language, path, qualname)));
    };
    if language.extraction().prefix != parsed.prefix() {
        let id = StableId::of(language, path, &symbol);
        let (prefix, name) = (parsed.prefix(), language.name());
        return Err(Missed::Reason(format!(
            "no {prefix} symbol in {path}: the file is {name}. This symbol's id is {id}"
        )));
    }
    let end_line = line_count(source).min(symbol.end_line);
    Ok(SymbolSpan {
        start_line: symbol.line,
        end_line,
        language,
        path: path.to_owned(),
        symbol,
        source: Arc::clone(source),
    })
}

// A data file keeps only an outline of its keys, so a key it still holds can miss.
fn undefined(language: Language, path: &str, qualname: &str) -> String {
    if is_data_language(language) {
        return SPAN_DATA_KEY_UNLISTED.render((path, qualname, OUTLINE_CAP));
    }
    format!("{path} no longer defines {qualname}")
}

// The path came from a caller, so a path that resolves outside the root is refused.
fn parse_file(reading: &Reading<'_>, path: &str) -> Noted<Result<ParsedFile, Missed>> {
    let absolute = match io::contained_path(&reading.repo.root, Path::new(path)) {
        Ok(absolute) => absolute,
        Err(refusal) => return Noted::new(Err(Missed::Refused(refusal))),
    };
    let bytes = match io::read_bounded(&absolute) {
        Ok(bytes) => bytes,
        Err(skipped) => return Noted::new(Err(Missed::Reason(format!("{path}: {skipped}")))),
    };
    let Some(language) = absolute.to_str().and_then(Language::from_path) else {
        let reason = format!("{path}: no grammar for this file type");
        return Noted::new(Err(Missed::Reason(reason)));
    };
    let file = FileText::decode(path.to_owned(), language, &bytes);
    let Served { value, warning } = reading.facts.symbols(&file, reading.deadline);
    let value = value
        .map(|symbols| ParsedFile {
            language,
            source: file.text,
            symbols,
        })
        .map_err(|unparsable| Missed::Reason(format!("{path}: {unparsable}")));
    let diagnostics = warning.map(|failure| failure.to_string());
    Noted {
        value,
        diagnostics: diagnostics.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests;
