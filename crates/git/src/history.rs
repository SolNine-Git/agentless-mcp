//! The history of a line span: the commits that touched it, whether a file differs from a commit,
//! and whether the clone is shallow.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;

use text::CodePointString;

use crate::{Git, GitOutcome, Limits};

/// The stdout cap of a span log. A commit message is small, so only a git that prints patches
/// or a rewritten log reaches it.
pub const MAX_HISTORY_OUTPUT_BYTES: usize = 2_000_000;

// The log walks history and computes a diff per commit, so it gets more time than a receipt call.
pub(crate) const HISTORY: Limits = Limits {
    seconds: 30,
    shown: "30.0",
    cap: MAX_HISTORY_OUTPUT_BYTES,
};
// diff --quiet and rev-parse print nothing or one word, so the cap bounds only a misbehaving git.
pub(crate) const PROBE: Limits = Limits {
    seconds: 5,
    shown: "5.0",
    cap: 4_096,
};

// %b has no terminator of its own, so with -z each commit is exactly four NUL-ended fields.
const FORMAT: &str = "--format=%H%x00%aI%x00%s%x00%b";
const FIELDS_PER_COMMIT: usize = 4;
const SHA1_DIGITS: usize = 40;
const SHA256_DIGITS: usize = 64;
const SHOWN_ANSWER_CHARS: usize = 80;

/// Lines `start` to `end` of a file, both counted from 1 and both included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRange {
    start: u32,
    end: u32,
}

impl LineRange {
    /// Returns the range, or `None` when `start` is 0 or `end` is before `start`.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Option<Self> {
        if start >= 1 && end >= start {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// Returns the first line.
    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    /// Returns the last line.
    #[must_use]
    pub const fn end(self) -> u32 {
        self.end
    }
}

/// A full object id: 40 lowercase hex digits, or 64 in a SHA-256 repository. No other text, so no
/// id can reach git as an option.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    /// Returns the id that `text` spells, or `None` for any other text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        is_full_sha(text).then(|| Self(text.to_owned()))
    }

    /// Returns the hex digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One span history request: the file, its lines, the commit to read them in, and how many
/// commits to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanQuery<'a> {
    /// The file, relative to the root that the call runs in.
    pub path: &'a str,
    /// The lines of the file in `revision`.
    pub lines: LineRange,
    /// The commit whose history git walks.
    pub revision: &'a CommitId,
    /// The most commits to list.
    pub max_count: NonZeroUsize,
}

/// One commit that touched a span: its full object id, author date, subject and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// The full object id: 40 hex digits, or 64 in a SHA-256 repository.
    pub sha: String,
    /// The author date in strict ISO 8601.
    pub authored: String,
    /// The subject line.
    pub subject: String,
    /// The body, as git printed it.
    pub body: String,
}

/// The commits that touched a span, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanLog {
    /// Each commit whose four fields git began to print, the last one possibly cut.
    pub commits: Vec<Commit>,
    /// Git printed past [`MAX_HISTORY_OUTPUT_BYTES`], so older commits can be missing.
    pub capped: bool,
}

/// Why a span log has no commits to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanLogError {
    /// Git gave no output. The note says why, and [`HistoryFailure::of`] classifies it.
    Unanswered(String),
    /// Git printed output that the pinned format cannot have written. The text names the defect.
    Malformed(String),
}

/// The kind of failure a git note describes, for the history refusals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryFailure {
    /// The path is not in the commit.
    NoPath,
    /// The span reaches past the end of the file in the commit.
    SpanBeyondHead,
    /// Git is not installed or could not start.
    NoGit,
    /// Any other failure.
    Other,
}

impl HistoryFailure {
    /// Classifies `note` by the wording of git log, git cat-file and the runner.
    #[must_use]
    pub fn of(note: &str) -> Self {
        let has = |words: &str| note.contains(words);
        if has("There is no path") || has("does not exist in") || has("but not in") {
            Self::NoPath
        } else if has("has only") && has("lines") {
            Self::SpanBeyondHead
        } else if has("not installed") || has("could not be run") {
            Self::NoGit
        } else {
            Self::Other
        }
    }
}

impl Git {
    /// Lists the commits that touched the span `query` names, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`SpanLogError`] when git gives no output, or output the format cannot have
    /// written.
    pub fn log_span(&self, root: &Path, query: &SpanQuery<'_>) -> Result<SpanLog, SpanLogError> {
        let SpanQuery {
            path,
            lines,
            revision,
            max_count,
        } = *query;
        // The path rides inside the -L token, where git reads it as a file name and never as a
        // pathspec.
        let range = OsString::from(format!("-L{},{}:{path}", lines.start, lines.end));
        let count = OsString::from(format!("--max-count={max_count}"));
        let rest = [
            range.as_os_str(),
            OsStr::new("--no-patch"),
            OsStr::new("-z"),
            OsStr::new(FORMAT),
            &count,
            OsStr::new(revision.as_str()),
            OsStr::new("--"),
        ];
        let (stdout, capped) =
            answer(self.run(root, "log", &rest, HISTORY)).map_err(SpanLogError::Unanswered)?;
        let text = String::from_utf8_lossy(&stdout);
        let commits = parse_log(&text, capped).map_err(SpanLogError::Malformed)?;
        Ok(SpanLog { commits, capped })
    }

    /// Says whether `path` differs from `revision`.
    ///
    /// # Errors
    ///
    /// Returns the note of a git that cannot say.
    pub fn differs(&self, root: &Path, path: &str, revision: &CommitId) -> Result<bool, String> {
        // "./" defeats pathspec magic, so a file named ":(exclude)x" is a path.
        let target = OsString::from(format!("./{path}"));
        let rest = [
            OsStr::new("--quiet"),
            OsStr::new(revision.as_str()),
            OsStr::new("--"),
            &target,
        ];
        match self.run(root, "diff", &rest, PROBE) {
            GitOutcome::Printed(_) => Ok(false),
            GitOutcome::Failed(failure) if failure.code == 1 => Ok(true),
            GitOutcome::Failed(failure) => Err(failure.to_string()),
            // A git killed at the cap has no exit status that answers, and no note.
            GitOutcome::Capped(_) => Err(String::new()),
            GitOutcome::Unfinished(unfinished) => Err(unfinished.to_string()),
        }
    }

    /// Says whether the repository is a shallow clone.
    ///
    /// # Errors
    ///
    /// Returns the note of a git that cannot say, or that printed neither "true" nor "false".
    pub fn is_shallow(&self, root: &Path) -> Result<bool, String> {
        let rest = [OsStr::new("--is-shallow-repository")];
        let (stdout, _) = answer(self.run(root, "rev-parse", &rest, PROBE))?;
        shallow_answer(&String::from_utf8_lossy(&stdout))
    }
}

// The stdout of a call and whether the cap cut it, or the note of a call that gave none.
fn answer(outcome: GitOutcome) -> Result<(Vec<u8>, bool), String> {
    match outcome {
        GitOutcome::Printed(stdout) => Ok((stdout, false)),
        GitOutcome::Capped(stdout) => Ok((stdout, true)),
        GitOutcome::Failed(failure) => Err(failure.to_string()),
        GitOutcome::Unfinished(unfinished) => Err(unfinished.to_string()),
    }
}

fn shallow_answer(text: &str) -> Result<bool, String> {
    match text::strip(text) {
        "true" => Ok(true),
        "false" => Ok(false),
        other => {
            let shown: String = other.chars().take(SHOWN_ANSWER_CHARS).collect();
            let quoted = CodePointString::from(shown.as_str()).quoted();
            Err(format!("git rev-parse printed {quoted}"))
        }
    }
}

// A cut log drops its partial last commit. A refusal names the defect and never echoes the field,
// so the bytes under suspicion stay out of the answer.
pub(crate) fn parse_log(text: &str, capped: bool) -> Result<Vec<Commit>, String> {
    let mut fields: Vec<&str> = text.split('\0').collect();
    if fields.last() == Some(&"") {
        fields.pop();
    }
    if !fields.len().is_multiple_of(FIELDS_PER_COMMIT) && !capped {
        let count = fields.len();
        return Err(format!(
            "{count} fields is not a whole number of {FIELDS_PER_COMMIT}-field commits"
        ));
    }
    let (whole, _) = fields.as_chunks::<FIELDS_PER_COMMIT>();
    whole
        .iter()
        .enumerate()
        .map(|(index, fields)| commit(index + 1, *fields))
        .collect()
}

fn commit(number: usize, fields: [&str; FIELDS_PER_COMMIT]) -> Result<Commit, String> {
    let [sha, authored, subject, body] = fields;
    if !is_full_sha(sha) {
        return Err(format!("commit {number} does not open with a full sha"));
    }
    if authored.is_empty() {
        return Err(format!("commit {number} carries no author date"));
    }
    Ok(Commit {
        sha: sha.to_owned(),
        authored: authored.to_owned(),
        subject: subject.to_owned(),
        body: body.to_owned(),
    })
}

fn is_full_sha(text: &str) -> bool {
    matches!(text.len(), SHA1_DIGITS | SHA256_DIGITS)
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests;
