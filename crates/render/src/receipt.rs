//! The receipt: which repository, at which commit, from which cache, under which config.

use std::path::Path;

use app::{Chars4, ConfigView, Session};
use io::ProjectConfig;
use prompts::envelope::{
    NOTICE, RECEIPT_CONFIG, RECEIPT_CONFIG_WARNING, RECEIPT_HEADER, RECEIPT_LINE, RECEIPT_NOTE,
    RECEIPT_SUMMARY,
};
use serde::Serialize;

use crate::count::Ceiling;
use crate::textsafe::one_line;

// No block but the body may take more than this share of the ceiling.
pub(crate) const BLOCK_SHARE: usize = 8;

/// The repository facts that head every answer, and the config warnings that follow its body.
#[derive(Debug)]
pub struct Receipt<'a> {
    pub(crate) root: &'a Path,
    pub(crate) head: Option<&'a str>,
    pub(crate) tree: Option<&'a str>,
    pub(crate) dirty: Option<usize>,
    pub(crate) cache: &'a str,
    pub(crate) note: &'a str,
    pub(crate) config: &'a ProjectConfig,
    pub(crate) warnings: Vec<String>,
}

impl<'a> Receipt<'a> {
    /// Returns the receipt of the repository, git state, cache and config that `session` holds.
    #[must_use]
    pub fn of(session: &'a Session) -> Receipt<'a> {
        let snapshot = &session.snapshot;
        Receipt {
            root: &session.repo.root,
            head: snapshot.head_sha.as_deref(),
            tree: snapshot.tree_oid.as_deref(),
            dirty: snapshot.dirty_count,
            cache: &session.cache.receipt,
            note: &snapshot.note,
            config: &session.repo.config,
            warnings: app::config_warnings(&session.repo.config),
        }
    }

    // The tool-authored lines. Each value can come from outside, so none may open a line.
    pub(crate) fn tool_lines(&self) -> Vec<String> {
        let root = self.root.display().to_string();
        let dirty = self
            .dirty
            .map_or_else(|| "unknown".to_owned(), |count| count.to_string());
        let mut lines = vec![
            RECEIPT_HEADER.to_owned(),
            RECEIPT_LINE.render((
                &one_line(&root),
                &one_line(self.head.unwrap_or("nogit")),
                &dirty,
                &one_line(self.cache),
            )),
        ];
        if !self.note.is_empty() {
            lines.push(RECEIPT_NOTE.render(&one_line(self.note)));
        }
        if let Some(path) = &self.config.path {
            let path = path.display().to_string();
            lines.push(RECEIPT_CONFIG.render(&one_line(&path)));
        }
        lines
    }

    // The JSON receipt. The config appears only when a file was read or a warning arose.
    pub(crate) fn fields(&self, ceiling: &Ceiling<'_>) -> Fields<'_> {
        let config =
            (self.config.path.is_some() || !self.warnings.is_empty()).then(|| ConfigView {
                config: self.config,
                warnings: bounded_warnings(&self.warnings, ceiling),
            });
        Fields {
            repo: self.root.display().to_string(),
            head: self.head,
            tree: self.tree,
            dirty: self.dirty,
            cache: self.cache,
            note: self.note,
            notice: NOTICE,
            config,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Fields<'a> {
    repo: String,
    head: Option<&'a str>,
    tree: Option<&'a str>,
    dirty: Option<usize>,
    cache: &'a str,
    note: &'a str,
    notice: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<ConfigView<'a>>,
}

/// Returns the receipt lines that a command writes to stderr: the tool lines, the summary line,
/// then the config warnings, bounded at the standard ceiling with the chars4 counter.
#[must_use]
pub fn receipt_lines(receipt: &Receipt<'_>, summary: &str) -> Vec<String> {
    let ceiling = Ceiling::standard(&Chars4);
    let mut lines = receipt.tool_lines();
    // The summary names repository content, so it gets the escape of every other value here.
    lines.push(RECEIPT_SUMMARY.render(&one_line(summary)));
    let warnings = bounded_warnings(&receipt.warnings, &ceiling);
    lines.extend(warnings.iter().map(|warning| warning_line(warning)));
    lines
}

pub(crate) fn warning_line(warning: &str) -> String {
    RECEIPT_CONFIG_WARNING.render(&one_line(warning))
}

// The first warnings that fit the count bound and the block share, in order. A skipped warning
// leaves a gap, and a count line says so. The counter measures the joined block each time.
pub(crate) fn bounded_warnings(warnings: &[String], ceiling: &Ceiling<'_>) -> Vec<String> {
    let limit = usize::try_from(app::MAX_CONFIG_WARNINGS).unwrap_or(usize::MAX);
    let budget = ceiling.tokens / BLOCK_SHARE;
    let mut kept = Vec::new();
    let mut block = String::new();
    for warning in warnings.iter().take(limit) {
        let probe = format!("{block}{}\n", warning_line(warning));
        if ceiling.count(&probe) > budget {
            continue;
        }
        block = probe;
        kept.push(warning.clone());
    }
    if kept.len() < warnings.len() {
        kept.push(suppressed(kept.len(), warnings.len()));
    }
    kept
}

fn suppressed(shown: usize, total: usize) -> String {
    format!("{shown} of {total} shown; the rest are suppressed and can be anywhere in the list")
}
