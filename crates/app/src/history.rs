//! Why a span exists: the commits that touched one symbol's lines, newest first, with their
//! bodies cut max-min fair to one token budget.

use std::num::NonZeroUsize;
use std::path::Path;

use facts::line_count;
use git::{CommitId, GitOutcome, HistoryFailure, LineRange, SpanLogError, SpanQuery};
use prompts::messages::{
    HISTORY_BODY_TRUNCATED, HISTORY_GIT_FAILED, HISTORY_GIT_OUTPUT_MALFORMED, HISTORY_NO_COMMITS,
    HISTORY_NO_GIT, HISTORY_OUTPUT_CAPPED_NO_COMMITS, HISTORY_PATH_NOT_IN_HEAD,
    HISTORY_SPAN_BEYOND_HEAD, HISTORY_SYMBOL_NOT_IN_HEAD, HISTORY_TARGET_UNRESOLVED,
};
use store::{FactSource, FileText};
use text::{one_line, splitlines, strip};

use crate::bounds::within;
use crate::budget::{TRUNCATION_MARKER_TOKENS, allocate, longest_fitting};
use crate::ops::limits::{MAX_BUDGET, MAX_LIMIT, MIN_BUDGET};
use crate::span::{Missed, SymbolSpan, resolve_symbol_span};
use crate::{Error, IntArg, Noted, ROW_INDENT, Reading, Services, TokenCounter};

// The budget cuts bodies and cannot cut a header row, so the row count needs its own cap.
const HISTORY_MAX_SEATS: usize = 120;
// A measured worst-case row is 32 tokens; the rest is room for the span header and the markers.
const HISTORY_TOKENS_PER_SEAT: usize = 40;
// A git subcommand that `git_text` runs, and the stdout cap it runs under.
type Call = (&'static str, usize);
const REV_PARSE: Call = ("rev-parse", 4_096);
const CAT_FILE: Call = ("cat-file", io::MAX_FILE_BYTES);
// The digits of the abbreviated sha that every row cites.
const SHORT_SHA_LENGTH: usize = 8;
const BODY_INDENT: &str = "    ";

/// A history request: the stable id, the most commits, and the token budget of the bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    /// The stable id of the symbol.
    pub target: String,
    /// The most commits the answer lists.
    pub limit: IntArg,
    /// The tokens the commit bodies share.
    pub budget: IntArg,
}

/// One commit on the span, with the body lines that the budget kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The full object id.
    pub sha: String,
    /// The author date in strict ISO 8601.
    pub authored: String,
    /// The subject line.
    pub subject: String,
    /// The body lines the budget kept, without leading or trailing blank lines.
    pub body_lines: Vec<String>,
    /// How many body lines the commit has.
    pub body_total: usize,
}

impl HistoryEntry {
    /// Returns the abbreviated sha that every row and marker cites this commit by.
    #[must_use]
    pub fn short_sha(&self) -> &str {
        self.sha.get(..SHORT_SHA_LENGTH).unwrap_or(&self.sha)
    }

    /// Returns how many body lines survived the budget.
    #[must_use]
    pub fn body_shown(&self) -> usize {
        self.body_lines.len()
    }
}

/// The commits that touched one span, newest first, and what the answer cannot vouch for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryResult {
    /// The stable id of the symbol in the working tree.
    pub target: String,
    /// The path below the repository root.
    pub path: String,
    /// The span's first line in the working tree.
    pub start_line: u32,
    /// The span's last line in the working tree.
    pub end_line: u32,
    /// The span's first line in the commit that git log read.
    pub committed_start_line: u32,
    /// The span's last line in that commit.
    pub committed_end_line: u32,
    /// The commits, newest first.
    pub entries: Vec<HistoryEntry>,
    /// Whether older commits touched the span, and which bound ended the list.
    pub list_end: ListEnd,
    /// Whether git printed past the output cap.
    pub output_capped: bool,
    /// Whether the file differs from HEAD, or `None` when git could not say.
    pub dirty: Option<bool>,
    /// Why git could not say whether the file differs.
    pub dirty_note: String,
    /// Whether the clone is shallow, or `None` when git could not say.
    pub shallow: Option<bool>,
    /// Why git could not say whether the clone is shallow.
    pub shallow_note: String,
    /// Whether the committed signature differs from the working tree's.
    pub signature_changed: bool,
}

/// Where the list of commits ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListEnd {
    /// No older commit touched the span.
    Whole,
    /// Older commits touched the span, past the caller's limit.
    AtLimit,
    /// Older commits touched the span, past the seats that the budget allows.
    AtSeats,
}

impl ListEnd {
    /// Returns whether older commits touched the span.
    #[must_use]
    pub fn more_commits(self) -> bool {
        self != ListEnd::Whole
    }

    /// Returns whether the budget's seat count, not the limit, ended the list.
    #[must_use]
    pub fn seats_capped(self) -> bool {
        self == ListEnd::AtSeats
    }
}

/// Returns the text of one entry: its row, its body lines indented, and the marker of a cut
/// body. The history budget measures this text.
#[must_use]
pub fn history_entry_text(entry: &HistoryEntry) -> String {
    let row = format!(
        "{}  {}  {}",
        one_line(entry.short_sha()),
        one_line(&entry.authored),
        one_line(&entry.subject)
    );
    let mut lines = vec![format!("{ROW_INDENT}{row}")];
    lines.extend(body_rows(&entry.body_lines));
    if entry.body_shown() < entry.body_total {
        let sha = one_line(entry.short_sha());
        let marker = HISTORY_BODY_TRUNCATED.render((&sha, entry.body_shown(), entry.body_total));
        lines.push(format!("{ROW_INDENT}{marker}"));
    }
    lines.join("\n") + "\n"
}

// Every body row is indented and escaped, so a marker in column 0 stays unforgeable.
fn body_rows(lines: &[String]) -> Vec<String> {
    let row = |line: &String| {
        if strip(line).is_empty() {
            String::new()
        } else {
            format!("{BODY_INDENT}{}", one_line(line))
        }
    };
    lines.iter().map(row).collect()
}

/// Lists the commits that touched the lines the target of `request` spans in the repository
/// that `reading` names. The cache's row warning comes with either outcome.
#[must_use]
pub fn history(
    services: &Services,
    reading: &Reading<'_>,
    request: &HistoryRequest,
) -> Noted<Result<HistoryResult, Error>> {
    let bounds = within(&request.limit, 1, MAX_LIMIT, "limit").and_then(|limit| {
        within(&request.budget, MIN_BUDGET, MAX_BUDGET, "budget").map(|budget| (limit, budget))
    });
    let (limit, budget) = match bounds {
        Ok(bounds) => bounds,
        Err(error) => return Noted::new(Err(error)),
    };
    let resolved = resolve_symbol_span(reading, &request.target);
    let span = resolved
        .value
        .map_err(|missed| unresolved(&request.target, missed));
    let value = span.and_then(|span| {
        let walk = Walk {
            services,
            reading,
            limit,
            budget,
        };
        walk.commits(&span)
    });
    Noted {
        value,
        diagnostics: resolved.diagnostics,
    }
}

fn unresolved(target: &str, missed: Missed) -> Error {
    let error = match missed {
        Missed::Reason(reason) => {
            kernel::Error::OperationFailed(HISTORY_TARGET_UNRESOLVED.render((target, &reason)))
        }
        Missed::Refused(refusal) => kernel::Error::SecurityRefusal(refusal.to_string()),
    };
    error.into()
}

fn failed(message: String) -> Error {
    kernel::Error::OperationFailed(message).into()
}

// One history call: the services, the repository and the caller's checked bounds.
struct Walk<'a> {
    services: &'a Services,
    reading: &'a Reading<'a>,
    limit: usize,
    budget: usize,
}

impl Walk<'_> {
    fn root(&self) -> &Path {
        &self.reading.repo.root
    }

    // git log -L reads line numbers against the commit, so a file that may differ from HEAD is
    // read as committed.
    fn commits(&self, working: &SymbolSpan) -> Result<HistoryResult, Error> {
        let Some(head) = self.reading.snapshot.head_sha.as_deref() else {
            return Err(failed(HISTORY_NO_GIT.render(&self.reading.snapshot.note)));
        };
        let commit = self.full_commit(head, working)?;
        let git = &self.services.git;
        let (dirty, dirty_note) = answered(git.differs(self.root(), &working.path, &commit));
        let span = if dirty == Some(false) {
            working.clone()
        } else {
            self.committed_span(&commit, working)?
        };
        let seats = (self.limit)
            .min(HISTORY_MAX_SEATS)
            .min((self.budget / HISTORY_TOKENS_PER_SEAT).max(1));
        let log = self.span_log(&span, &commit, seats)?;
        let (shallow, shallow_note) = answered(git.is_shallow(self.root()));
        let list_end = match log.commits.len() > seats {
            false => ListEnd::Whole,
            true if seats < self.limit => ListEnd::AtSeats,
            true => ListEnd::AtLimit,
        };
        let entries = log.commits.into_iter().take(seats).map(entry).collect();
        Ok(HistoryResult {
            target: working.stable_id().to_string(),
            path: span.path.clone(),
            start_line: working.start_line,
            end_line: working.end_line,
            committed_start_line: span.start_line,
            committed_end_line: span.end_line,
            entries: fit(&*self.services.counter, entries, self.budget),
            list_end,
            output_capped: log.capped,
            dirty,
            dirty_note,
            shallow,
            shallow_note,
            signature_changed: span.symbol.signature != working.symbol.signature,
        })
    }

    // The full id of the receipt's short HEAD sha.
    fn full_commit(&self, head: &str, span: &SymbolSpan) -> Result<CommitId, Error> {
        let printed = git_text(self.services.git.verify(self.root(), head), span, REV_PARSE)?;
        let full = git::parse_commit(&printed)
            .map_err(|malformed| failed(HISTORY_GIT_FAILED.render(&malformed.to_string())))?;
        CommitId::parse(&full).ok_or_else(|| {
            let note = git::Malformed::NotOneObjectName.to_string();
            failed(HISTORY_GIT_FAILED.render(&note))
        })
    }

    // The symbol as `commit` has it, by its qualified name in the working tree.
    fn committed_span(&self, commit: &CommitId, span: &SymbolSpan) -> Result<SymbolSpan, Error> {
        let blob = self
            .services
            .git
            .blob(self.root(), commit.as_str(), &span.path);
        let bytes = git_text(blob, span, CAT_FILE)?;
        let file = FileText::decode(span.path.clone(), span.language, &bytes);
        let symbols = FactSource::parsing().symbols(&file).value;
        let symbols =
            symbols.map_err(|unparsable| failed(format!("{}: {unparsable}", span.path)))?;
        let wanted = span.symbol.id_qualname();
        let Some(symbol) = symbols
            .into_iter()
            .find(|symbol| symbol.id_qualname() == wanted)
        else {
            let message = HISTORY_SYMBOL_NOT_IN_HEAD.render((&span.path, &wanted));
            return Err(failed(message));
        };
        Ok(SymbolSpan {
            start_line: symbol.line,
            end_line: line_count(&file.text).min(symbol.end_line),
            symbol,
            language: span.language,
            path: span.path.clone(),
            source: file.text,
        })
    }

    fn span_log(
        &self,
        span: &SymbolSpan,
        commit: &CommitId,
        seats: usize,
    ) -> Result<git::SpanLog, Error> {
        let lines = line_range(span.start_line, span.end_line)?;
        let max_count = NonZeroUsize::MIN.saturating_add(seats);
        let query = SpanQuery {
            path: &span.path,
            lines,
            revision: commit,
            max_count,
        };
        let log = self.services.git.log_span(self.root(), &query);
        let log = log.map_err(|error| match error {
            SpanLogError::Unanswered(note) => failed(failure(&note, span)),
            SpanLogError::Malformed(detail) => failed(HISTORY_GIT_OUTPUT_MALFORMED.render(&detail)),
        })?;
        if log.commits.is_empty() {
            return Err(failed(no_commits(span, log.capped)));
        }
        Ok(log)
    }
}

fn line_range(start: u32, end: u32) -> Result<LineRange, Error> {
    if start < 1 {
        return Err(failed(format!("start must be at least 1, got {start}")));
    }
    LineRange::new(start, end)
        .ok_or_else(|| failed(format!("end must be at least {start}, got {end}")))
}

// A git that cannot say gives `None` with its note.
fn answered(outcome: Result<bool, String>) -> (Option<bool>, String) {
    match outcome {
        Ok(answer) => (Some(answer), String::new()),
        Err(note) => (None, note),
    }
}

// The stdout of a git call, refused when git printed past the call's cap or gave no output.
fn git_text(outcome: GitOutcome, span: &SymbolSpan, call: Call) -> Result<Vec<u8>, Error> {
    let (subcommand, cap) = call;
    let note = match outcome {
        GitOutcome::Printed(stdout) => return Ok(stdout),
        GitOutcome::Capped(_) => {
            let note = format!("git {subcommand} printed more than {cap} bytes");
            return Err(failed(HISTORY_GIT_FAILED.render(&note)));
        }
        GitOutcome::Failed(refused) => refused.to_string(),
        GitOutcome::Unfinished(unfinished) => unfinished.to_string(),
    };
    Err(failed(failure(&note, span)))
}

fn failure(note: &str, span: &SymbolSpan) -> String {
    let (start, end) = (span.start_line as usize, span.end_line as usize);
    match HistoryFailure::of(note) {
        HistoryFailure::NoPath => HISTORY_PATH_NOT_IN_HEAD.render(&span.path),
        HistoryFailure::SpanBeyondHead => HISTORY_SPAN_BEYOND_HEAD.render((&span.path, start, end)),
        HistoryFailure::NoGit => HISTORY_NO_GIT.render(note),
        HistoryFailure::Other => HISTORY_GIT_FAILED.render(note),
    }
}

fn no_commits(span: &SymbolSpan, capped: bool) -> String {
    let (start, end) = (span.start_line as usize, span.end_line as usize);
    if capped {
        let bytes = git::MAX_HISTORY_OUTPUT_BYTES;
        HISTORY_OUTPUT_CAPPED_NO_COMMITS.render((&span.path, start, end, bytes))
    } else {
        HISTORY_NO_COMMITS.render((&span.path, start, end))
    }
}

// The body without its leading and trailing blank lines.
fn entry(commit: git::Commit) -> HistoryEntry {
    let lines = splitlines(&commit.body, false);
    let blank = |line: &&str| strip(line).is_empty();
    let first = lines
        .iter()
        .position(|line| !blank(line))
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|line| !blank(line))
        .map_or(first, |at| at + 1);
    let body_lines: Vec<String> = lines[first..last]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    HistoryEntry {
        body_total: body_lines.len(),
        sha: commit.sha,
        authored: commit.authored,
        subject: commit.subject,
        body_lines,
    }
}

// Entries that fit an equal share stay whole; the others are cut to the last share.
fn fit(counter: &dyn TokenCounter, entries: Vec<HistoryEntry>, budget: usize) -> Vec<HistoryEntry> {
    let costs: Vec<usize> = entries
        .iter()
        .map(|entry| counter.count(&history_entry_text(entry)))
        .collect();
    let (cut, share) = allocate(&costs, budget);
    (entries.into_iter().enumerate())
        .map(|(index, entry)| {
            if cut.contains(&index) {
                shorten(counter, entry, share)
            } else {
                entry
            }
        })
        .collect()
}

// The header row always renders, so a body may go to nothing.
fn shorten(counter: &dyn TokenCounter, entry: HistoryEntry, share: usize) -> HistoryEntry {
    if entry.body_lines.is_empty() {
        return entry;
    }
    let bare = HistoryEntry {
        body_lines: Vec::new(),
        body_total: 0,
        ..entry.clone()
    };
    let header = counter.count(&history_entry_text(&bare));
    let room = share.checked_sub(header + TRUNCATION_MARKER_TOKENS);
    let lines = &entry.body_lines;
    let fits = |kept: usize| {
        let rows = body_rows(&lines[..kept]).join("\n");
        room.is_some_and(|room| counter.count(&rows) <= room)
    };
    let kept = longest_fitting(0, lines.len(), fits);
    HistoryEntry {
        body_lines: lines[..kept].to_vec(),
        ..entry
    }
}

#[cfg(test)]
mod tests;
