//! The text of the tool answers whose body differs from the command line's: an expansion that
//! lists the ids that missed, an overview that keeps the block of each file that failed, and a
//! slice that shows why it has no text.

use app::{ExpandResult, FileView, ROW_INDENT};
use text::one_line;

use crate::Receipt;
use crate::envelope::wrap;
use crate::symbols::{expand_stderr_rows, expansion_text};
use crate::views::{overview_block, overview_header};

/// A view in the form that an MCP tool answers with.
#[derive(Clone, Copy, Debug)]
pub enum ToolForm<'a> {
    /// The cards and the shortening note, then one row for each id that did not resolve.
    Expand(&'a ExpandResult),
    /// Each file's block in order, with the reason under the header of a file that failed.
    Overview(&'a [FileView]),
    /// The numbered lines, or the reason when the slice has none.
    Slice(&'a FileView),
}

/// Returns `form` as text under its receipt, cut at the token ceiling.
#[must_use]
pub fn tool_text(receipt: &Receipt<'_>, form: ToolForm<'_>) -> String {
    let body = match form {
        ToolForm::Expand(result) => expand_body(result),
        ToolForm::Overview(views) => overview_body(views),
        ToolForm::Slice(view) => slice_body(view),
    };
    wrap(receipt, &body, &receipt.ceiling(), None)
}

fn expand_body(result: &ExpandResult) -> String {
    let mut lines = vec![expansion_text(result)];
    lines.extend(expand_stderr_rows(result));
    lines.join("\n")
}

fn overview_body(views: &[FileView]) -> String {
    let blocks: Vec<String> = views.iter().map(tool_block).collect();
    blocks.join("\n")
}

// A file that failed keeps its header, with the reason indented below it.
fn tool_block(view: &FileView) -> String {
    if view.error.is_empty() {
        return overview_block(view);
    }
    let header = overview_header(view);
    format!("{header}\n{ROW_INDENT}{}", one_line(&view.error))
}

fn slice_body(view: &FileView) -> String {
    if view.text.is_empty() {
        view.error.clone()
    } else {
        view.text.clone()
    }
}
