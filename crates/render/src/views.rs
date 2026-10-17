//! The file views' two forms: overview blocks, numbered slices and resolved locations in the
//! text, and each view's fields in the JSON.

use app::{FileView, LocationView, ROW_INDENT};
use facts::StableId;
use prompts::messages::OVERVIEW_STABLE_IDS;
use serde::Serialize;
use syntax::Language;
use text::one_line;

use crate::envelope::{Listing, wrap_json, wrap_json_whole};
use crate::{Rendered, View};

// The language a header names when no grammar reads the file.
const UNKNOWN_LANGUAGE: &str = "unknown";
// The qualified-name placeholder of the id pattern under an overview header.
const QUALIFIED_NAME: &str = "<QualifiedName>";

impl View for Vec<FileView> {
    fn rendered(&self) -> Rendered<'_> {
        let blocks: Vec<String> = self
            .iter()
            .filter(|view| view.error.is_empty())
            .map(overview_block)
            .collect();
        let payload = FilesJson {
            files: self.iter().map(FileJson::of).collect(),
        };
        Rendered::new(blocks.join("\n"), payload, None, wrap_json)
    }
}

impl View for FileView {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(self.text.clone(), FileJson::of(self), None, wrap_json_whole)
    }
}

impl View for LocationView {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(
            locations_text(self),
            LocationJson::of(self),
            None,
            wrap_json_whole,
        )
    }
}

// One file's header, the pattern of its stable ids, then its skeleton.
pub(crate) fn overview_block(view: &FileView) -> String {
    let header = overview_header(view);
    let prefix = Language::from_name(&view.language).map_or("", |known| known.extraction().prefix);
    let pattern = StableId::new(prefix, &view.path, QUALIFIED_NAME).to_string();
    let ids = OVERVIEW_STABLE_IDS.render(&one_line(&pattern));
    format!("{header}\n{ROW_INDENT}{ids}\n{}", view.text)
}

// A file's header: its path, then its language, or "unknown" when no grammar reads the file.
pub(crate) fn overview_header(view: &FileView) -> String {
    let language = if view.language.is_empty() {
        UNKNOWN_LANGUAGE.to_owned()
    } else {
        one_line(&view.language).into_owned()
    };
    format!("{}  ({language})", one_line(&view.path))
}

// The ids, the intervals and the misses, then the numbered lines.
fn locations_text(view: &LocationView) -> String {
    let resolution = &view.resolution;
    let mut lines = vec![format!("file: {}", view.path)];
    lines.extend(
        resolution
            .stable_ids
            .iter()
            .map(|id| format!("matched: {id}")),
    );
    let intervals: Vec<String> = resolution
        .intervals
        .iter()
        .map(|(start, end)| format!("{start}-{end}"))
        .collect();
    let intervals = if intervals.is_empty() {
        "none".to_owned()
    } else {
        intervals.join(", ")
    };
    lines.push(format!("intervals: {intervals}"));
    let misses = resolution.unrecognized.iter();
    lines.extend(misses.map(|miss| format!("unrecognized: {} -- {}", miss.loc, miss.reason)));
    if !view.text.is_empty() {
        lines.push(String::new());
        lines.push(view.text.trim_end_matches('\n').to_owned());
    }
    lines.join("\n") + "\n"
}

#[derive(Clone, Serialize)]
struct FilesJson<'a> {
    files: Vec<FileJson<'a>>,
}

impl Listing for FilesJson<'_> {
    fn items(&self) -> usize {
        self.files.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.files.truncate(kept);
        prefix
    }
}

// The error and the refusal mark appear only when they hold something.
#[derive(Clone, Serialize)]
struct FileJson<'a> {
    path: &'a str,
    language: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refused: Option<bool>,
}

impl<'a> FileJson<'a> {
    fn of(view: &'a FileView) -> Self {
        FileJson {
            path: &view.path,
            language: &view.language,
            text: &view.text,
            error: Some(view.error.as_str()).filter(|error| !error.is_empty()),
            refused: view.refused.then_some(true),
        }
    }
}

#[derive(Serialize)]
struct LocationJson<'a> {
    path: &'a str,
    stable_ids: Vec<String>,
    spans: &'a [(u32, u32)],
    intervals: &'a [(u32, u32)],
    unrecognized: Vec<MissJson<'a>>,
    slice: &'a str,
}

#[derive(Serialize)]
struct MissJson<'a> {
    loc: &'a str,
    reason: &'a str,
}

impl<'a> LocationJson<'a> {
    fn of(view: &'a LocationView) -> Self {
        let resolution = &view.resolution;
        LocationJson {
            path: &view.path,
            stable_ids: resolution
                .stable_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            spans: &resolution.spans,
            intervals: &resolution.intervals,
            unrecognized: resolution
                .unrecognized
                .iter()
                .map(|miss| MissJson {
                    loc: &miss.loc,
                    reason: &miss.reason,
                })
                .collect(),
            slice: &view.text,
        }
    }
}
