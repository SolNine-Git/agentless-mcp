//! The search answer's two forms: the match lines grouped by file and by the symbol that holds
//! them, with noise files as count lines, or the rows of a file listing, in the text; and the
//! typed values in the JSON, trimmed at the file groups or the rows.

use app::search::{
    Block, FileCount, FileMatches, Line, Listing as FileListing, Matches, Noise, Owner,
    PatternFault, Repeat, SearchError, SearchRequest, Searched,
};
use app::{ROW_INDENT, Skipped, locator};
use facts::line_prefix;
use prompts::messages::{
    LISTED_AND, SEARCH_BAD_PATTERN, SEARCH_BAD_PATTERN_NO_COLUMN, SEARCH_CUT, SEARCH_GLOB_ROOT,
    SEARCH_LISTING, SEARCH_LISTING_NO_FILE, SEARCH_LISTING_REST, SEARCH_MORE_FILES, SEARCH_NO_FILE,
    SEARCH_NOISE, SEARCH_NOTHING_ASKED, SEARCH_PATTERN_TOO_BIG, SEARCH_REPEAT, SEARCH_REST,
    SEARCH_SCOPE_FILES, SEARCH_SCOPE_PATH, SEARCH_SKIPPED, SEARCH_SUMMARY, SEARCH_SUMMARY_NONE,
    SEARCH_WIDE_GLOB, TRACE_UNKNOWN_PATH_NONE,
};
use serde::Serialize;
use text::{MAX_BRACE_GLOBS, one_line};

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json};
use crate::trace::{PlaceJson, RowJson, listing_row, places, span};
use crate::{Rendered, View};

// The flag that a case-insensitive pattern carries after its closing slash.
const IGNORE_CASE: char = 'i';
// The text names at most this many files of one count line. The JSON names every one.
const FILES_SHOWN: usize = 5;
// The gutter of a repeated line, whose numbers are in its places.
const REPEAT_GUTTER: &str = "| ";

/// A search answer: the request, whose pattern, path and glob the summary repeats, and what the
/// search found.
#[derive(Clone, Copy, Debug)]
pub struct SearchAnswer<'a> {
    /// The request.
    pub request: &'a SearchRequest,
    /// The match lines, or the rows of a file listing.
    pub searched: &'a Searched,
}

impl View for SearchAnswer<'_> {
    fn rendered(&self) -> Rendered<'_> {
        let payload = SearchJson::of(self.request, self.searched);
        Rendered::new(search_text(self), payload, None, wrap_json)
    }
}

/// Returns the answer with no receipt: the summary line, then the matches or the file rows.
#[must_use]
pub fn search_text(answer: &SearchAnswer<'_>) -> String {
    let lines = match answer.searched {
        Searched::Matches(matches) => matches_lines(answer.request, matches),
        Searched::Listing(listing) => listing_lines(answer.request, listing),
        Searched::NoFile => vec![no_file_line(answer.request)],
    };
    lines.join("\n") + "\n"
}

/// Returns the refusal text of a search that gives no answer. A refusal of the bounds, the
/// containment, the walk or the deadline keeps its own message.
#[must_use]
pub fn search_refusal(error: &SearchError) -> String {
    match error {
        SearchError::Pattern(refused) => {
            let escaped = one_line(&refused.escaped);
            match &refused.fault {
                PatternFault::Syntax {
                    reason,
                    column: Some(column),
                } => SEARCH_BAD_PATTERN.render((&escaped, *column, &one_line(reason))),
                PatternFault::Syntax {
                    reason,
                    column: None,
                }
                | PatternFault::Other(reason) => {
                    SEARCH_BAD_PATTERN_NO_COLUMN.render((&escaped, &one_line(reason)))
                }
                PatternFault::TooBig { limit } => SEARCH_PATTERN_TOO_BIG.render((&escaped, *limit)),
            }
        }
        SearchError::NothingAsked => SEARCH_NOTHING_ASKED.to_owned(),
        SearchError::WideGlob { glob } => {
            SEARCH_WIDE_GLOB.render((MAX_BRACE_GLOBS, &one_line(glob)))
        }
        SearchError::UnknownPath { path } => TRACE_UNKNOWN_PATH_NONE.render(&one_line(path)),
        SearchError::Refused(error) => one_line(&error.to_string()).into_owned(),
    }
}

// The part of the summary after the pattern: the case flag, the path and the glob.
fn scope(request: &SearchRequest, glob: bool) -> String {
    let mut scope = String::new();
    if request.ignore_case && request.pattern.is_some() {
        scope.push(IGNORE_CASE);
    }
    if let Some(path) = &request.path {
        scope.push_str(&SEARCH_SCOPE_PATH.render(&one_line(path)));
    }
    if let Some(files) = request.files.as_deref().filter(|_| glob) {
        scope.push_str(&SEARCH_SCOPE_FILES.render(&one_line(files)));
    }
    scope
}

fn matches_lines(request: &SearchRequest, matches: &Matches) -> Vec<String> {
    let scope = scope(request, true);
    let pattern = one_line(request.pattern.as_deref().unwrap_or_default());
    if matches.matches == 0 {
        return vec![SEARCH_SUMMARY_NONE.render((&scope, &pattern, matches.searched))];
    }
    let summary = (&*scope, &*pattern, matches.matches, matches.files);
    let mut lines = vec![SEARCH_SUMMARY.render(summary)];
    lines.extend(matches.groups.iter().flat_map(group_lines));
    lines.extend(matches.repeats.iter().flat_map(repeat_lines));
    lines.extend(matches.noise.iter().map(noise_line));
    lines.extend(rest_line(matches));
    lines.extend(skipped_line(&matches.skipped));
    lines
}

// A file's path, then each block: the symbol that holds its lines, when one does, and the lines.
fn group_lines(group: &FileMatches) -> Vec<String> {
    let mut lines = vec![one_line(&group.path).into_owned()];
    for block in &group.blocks {
        lines.extend(block.owner.as_ref().map(owner_line));
        lines.extend(block.lines.iter().map(numbered));
    }
    lines
}

fn owner_line(owner: &Owner) -> String {
    let span = span(owner.start_line, owner.end_line);
    let id = locator(&owner.stable_id, "", Some(&span));
    format!("{ROW_INDENT}{}  {id}", one_line(&owner.qualname))
}

fn numbered(line: &Line) -> String {
    let text = one_line(&line.text);
    let text = match line.cut {
        Some(chars) => SEARCH_CUT.render((chars, &text)),
        None => text.into_owned(),
    };
    format!("{ROW_INDENT}{ROW_INDENT}{}{text}", line_prefix(line.number))
}

fn repeat_lines(repeat: &Repeat) -> [String; 2] {
    let places: Vec<String> = (repeat.places.iter())
        .map(|place| format!("{} @{}", one_line(&place.path), place.line))
        .collect();
    let head = SEARCH_REPEAT.render((repeat.places.len(), &places.join(", ")));
    let text = one_line(&repeat.text);
    [
        head,
        format!("{ROW_INDENT}{ROW_INDENT}{REPEAT_GUTTER}{text}"),
    ]
}

fn noise_line(noise: &Noise) -> String {
    let listed = counted_files(&noise.files);
    SEARCH_NOISE.render((
        noise.class.as_str(),
        noise.matches,
        noise.files.len(),
        &listed,
    ))
}

fn rest_line(matches: &Matches) -> Option<String> {
    if matches.rest.is_empty() {
        return None;
    }
    let left: usize = matches.rest.iter().map(|file| file.count).sum();
    let listed = counted_files(&matches.rest);
    let values = (
        matches.shown,
        matches.rest.len(),
        matches.shown + left,
        &*listed,
    );
    Some(SEARCH_REST.render(values))
}

// The first files with their match counts, then how many more there are.
fn counted_files(files: &[FileCount]) -> String {
    let shown: Vec<String> = (files.iter().take(FILES_SHOWN))
        .map(|file| format!("{} ({})", one_line(&file.path), file.count))
        .collect();
    more_files(&shown, files.len())
}

fn more_files(shown: &[String], total: usize) -> String {
    let rest = total - shown.len();
    let listed = shown.join(", ");
    if rest == 0 {
        listed
    } else {
        LISTED_AND.render((&SEARCH_MORE_FILES.render(rest), &listed))
    }
}

fn skipped_line(skipped: &[Skipped]) -> Option<String> {
    if skipped.is_empty() {
        return None;
    }
    let shown: Vec<String> = (skipped.iter().take(FILES_SHOWN))
        .map(|entry| format!("{} ({})", one_line(&entry.path), one_line(&entry.reason)))
        .collect();
    let listed = more_files(&shown, skipped.len());
    Some(SEARCH_SKIPPED.render((skipped.len(), &listed)))
}

// The request's summary, then the rule that the glob missed.
fn no_file_line(request: &SearchRequest) -> String {
    let head = if let Some(pattern) = &request.pattern {
        SEARCH_NO_FILE.render((&scope(request, true), &one_line(pattern)))
    } else {
        let glob = one_line(request.files.as_deref().unwrap_or_default());
        SEARCH_LISTING_NO_FILE.render((&scope(request, false), &glob))
    };
    format!("{head} {SEARCH_GLOB_ROOT}")
}

fn listing_lines(request: &SearchRequest, listing: &FileListing) -> Vec<String> {
    let glob = one_line(request.files.as_deref().unwrap_or_default());
    let head = SEARCH_LISTING.render((&scope(request, false), listing.files, &glob));
    let mut lines = vec![head];
    lines.extend(listing.rows.iter().map(listing_row));
    if listing.files > listing.rows.len() {
        lines.push(SEARCH_LISTING_REST.render((listing.rows.len(), listing.files)));
    }
    lines
}

// The request beside the answer, so that a reader of the document knows what it answers.
#[derive(Clone, Serialize)]
struct RequestJson<'a> {
    pattern: Option<&'a str>,
    files: Option<&'a str>,
    path: Option<&'a str>,
    ignore_case: bool,
}

#[derive(Clone, Serialize)]
struct SearchJson<'a> {
    request: RequestJson<'a>,
    #[serde(flatten)]
    answer: AnswerJson<'a>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
enum AnswerJson<'a> {
    Matches(MatchesJson<'a>),
    Listing {
        files: usize,
        rows: Vec<RowJson<'a>>,
    },
    NoFile,
}

impl<'a> SearchJson<'a> {
    fn of(request: &'a SearchRequest, searched: &'a Searched) -> Self {
        let answer = match searched {
            Searched::Matches(matches) => AnswerJson::Matches(MatchesJson::of(matches)),
            Searched::Listing(listing) => AnswerJson::Listing {
                files: listing.files,
                rows: listing.rows.iter().map(RowJson::of).collect(),
            },
            Searched::NoFile => AnswerJson::NoFile,
        };
        SearchJson {
            request: RequestJson {
                pattern: request.pattern.as_deref(),
                files: request.files.as_deref(),
                path: request.path.as_deref(),
                ignore_case: request.ignore_case,
            },
            answer,
        }
    }
}

// The ceiling drops whole file groups, or whole rows of a listing.
impl Listing for SearchJson<'_> {
    fn items(&self) -> usize {
        match &self.answer {
            AnswerJson::Matches(matches) => matches.groups.len(),
            AnswerJson::Listing { rows, .. } => rows.len(),
            AnswerJson::NoFile => 0,
        }
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        match &mut prefix.answer {
            AnswerJson::Matches(matches) => matches.groups.truncate(kept),
            AnswerJson::Listing { rows, .. } => rows.truncate(kept),
            AnswerJson::NoFile => {}
        }
        prefix
    }
}

#[derive(Clone, Serialize)]
struct MatchesJson<'a> {
    matches: usize,
    files: usize,
    searched: usize,
    shown: usize,
    groups: Vec<GroupJson<'a>>,
    repeats: Vec<RepeatJson<'a>>,
    rest: Vec<CountJson<'a>>,
    noise: Vec<NoiseJson<'a>>,
    skipped: Vec<SkippedJson<'a>>,
}

impl<'a> MatchesJson<'a> {
    fn of(matches: &'a Matches) -> Self {
        MatchesJson {
            matches: matches.matches,
            files: matches.files,
            searched: matches.searched,
            shown: matches.shown,
            groups: matches.groups.iter().map(GroupJson::of).collect(),
            repeats: matches.repeats.iter().map(RepeatJson::of).collect(),
            rest: matches.rest.iter().map(CountJson::of).collect(),
            noise: matches.noise.iter().map(NoiseJson::of).collect(),
            skipped: skipped_json(&matches.skipped),
        }
    }
}

#[derive(Clone, Serialize)]
struct GroupJson<'a> {
    path: &'a str,
    blocks: Vec<BlockJson<'a>>,
}

impl<'a> GroupJson<'a> {
    fn of(group: &'a FileMatches) -> Self {
        GroupJson {
            path: &group.path,
            blocks: group.blocks.iter().map(BlockJson::of).collect(),
        }
    }
}

#[derive(Clone, Serialize)]
struct BlockJson<'a> {
    owner: Option<OwnerJson<'a>>,
    lines: Vec<LineJson<'a>>,
}

impl<'a> BlockJson<'a> {
    fn of(block: &'a Block) -> Self {
        let owner = block.owner.as_ref().map(|owner| OwnerJson {
            qualname: &owner.qualname,
            stable_id: &owner.stable_id,
            start_line: owner.start_line,
            end_line: owner.end_line,
        });
        let lines = block.lines.iter().map(|line| LineJson {
            line: line.number,
            text: &line.text,
            matched: line.matched,
            cut: line.cut,
        });
        BlockJson {
            owner,
            lines: lines.collect(),
        }
    }
}

#[derive(Clone, Serialize)]
struct OwnerJson<'a> {
    qualname: &'a str,
    stable_id: &'a str,
    start_line: u32,
    end_line: u32,
}

#[derive(Clone, Serialize)]
struct LineJson<'a> {
    line: u32,
    text: &'a str,
    matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cut: Option<usize>,
}

#[derive(Clone, Serialize)]
struct RepeatJson<'a> {
    text: &'a str,
    places: Vec<PlaceJson<'a>>,
}

impl<'a> RepeatJson<'a> {
    fn of(repeat: &'a Repeat) -> Self {
        RepeatJson {
            text: &repeat.text,
            places: places(&repeat.places),
        }
    }
}

#[derive(Clone, Serialize)]
struct CountJson<'a> {
    path: &'a str,
    count: usize,
}

impl<'a> CountJson<'a> {
    fn of(file: &'a FileCount) -> Self {
        CountJson {
            path: &file.path,
            count: file.count,
        }
    }
}

#[derive(Clone, Serialize)]
struct NoiseJson<'a> {
    class: &'static str,
    matches: usize,
    files: Vec<CountJson<'a>>,
}

impl<'a> NoiseJson<'a> {
    fn of(noise: &'a Noise) -> Self {
        NoiseJson {
            class: noise.class.as_str(),
            matches: noise.matches,
            files: noise.files.iter().map(CountJson::of).collect(),
        }
    }
}
