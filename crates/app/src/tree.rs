//! The directory tree view: the files that the repository walk lists, as an indented tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use git::RepoFile;
use text::one_line;

use crate::bounds::within;
use crate::ops::limits::{MAX_TREE_DEPTH, MAX_TREE_ENTRIES};
use crate::{Error, IntArg, Noted, Repo, Services};

/// A tree request: the levels to render and the names to render before the cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRequest {
    /// The directory levels rendered below the root.
    pub depth: IntArg,
    /// The files and directories rendered before the tree reports its cut.
    pub max_entries: IntArg,
}

/// A rendered directory tree and the counts behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeView {
    /// The indented tree, with a trailing line when entries were cut.
    pub text: String,
    /// The number of files the walk listed.
    pub file_count: usize,
    /// The depth the tree was rendered to.
    pub depth: usize,
    /// The entry count the tree was cut at.
    pub max_entries: usize,
}

/// Renders the walk of `repo` as a tree. The walk's warnings come with either outcome.
#[must_use]
pub fn tree(
    services: &Services,
    repo: &Repo,
    request: &TreeRequest,
) -> Noted<Result<TreeView, Error>> {
    subtree(services, repo, None, request)
}

/// Renders the walk of the directory that `path` names inside `repo`, or of the root for `None`,
/// as a tree. The walk's warnings come with either outcome.
#[must_use]
pub fn subtree(
    services: &Services,
    repo: &Repo,
    path: Option<&str>,
    request: &TreeRequest,
) -> Noted<Result<TreeView, Error>> {
    let checked = within(&request.depth, 1, MAX_TREE_DEPTH, "depth").and_then(|depth| {
        within(&request.max_entries, 1, MAX_TREE_ENTRIES, "max_entries")
            .map(|entries| (depth, entries))
    });
    let selected = checked.and_then(|bounds| Ok((bounds, selected_directory(&repo.root, path)?)));
    let ((depth, max_entries), selected) = match selected {
        Ok(found) => found,
        Err(error) => return Noted::new(Err(error)),
    };
    let walk = services.git.walk(&selected);
    let diagnostics = walk.warnings.iter().map(ToString::to_string).collect();
    let value = walk
        .files
        .map_err(|refusal| kernel::Error::from(refusal).into())
        .map(|files| TreeView {
            text: render_tree(&files, depth, max_entries),
            file_count: files.len(),
            depth,
            max_entries,
        });
    Noted { value, diagnostics }
}

// The root for `None`, else the directory that `path` names inside the root.
fn selected_directory(root: &Path, path: Option<&str>) -> Result<PathBuf, Error> {
    let Some(path) = path else {
        return Ok(root.to_path_buf());
    };
    let selected = io::contained_path(root, Path::new(path)).map_err(kernel::Error::from)?;
    if selected.is_dir() {
        return Ok(selected);
    }
    let relative = selected.strip_prefix(root).unwrap_or(&selected);
    let message = format!("not a directory in this repository: {}", relative.display());
    Err(kernel::Error::RepoResolution(message).into())
}

// A directory: each name maps to its subdirectory, or to `None` for a file.
#[derive(Default)]
struct Directory {
    entries: BTreeMap<String, Option<Directory>>,
}

struct Budget {
    remaining: usize,
    omitted: usize,
}

// Four spaces per level, a slash after each directory, a "..." line under a directory past the
// depth, and a closing line that counts the names past the entry budget.
fn render_tree(files: &[RepoFile], depth: usize, max_entries: usize) -> String {
    let tree = build_tree(files);
    let mut lines = Vec::new();
    let mut budget = Budget {
        remaining: max_entries,
        omitted: 0,
    };
    render_level(&tree, 0, depth, &mut budget, &mut lines);
    if budget.omitted > 0 {
        let omitted = budget.omitted;
        lines.push(format!(
            "... {omitted} more entries truncated (max_entries={max_entries})"
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n") + "\n"
}

// A name met as a file and then as a directory becomes the directory, and a file never replaces a
// directory.
fn build_tree(files: &[RepoFile]) -> Directory {
    let mut root = Directory::default();
    for file in files {
        let mut parts: Vec<&str> = file.path.split('/').collect();
        let leaf = parts.pop().unwrap_or_default();
        let mut cursor = &mut root;
        for part in parts {
            let slot = cursor.entries.entry(part.to_owned()).or_insert(None);
            cursor = slot.get_or_insert_with(Directory::default);
        }
        cursor.entries.entry(leaf.to_owned()).or_insert(None);
    }
    root
}

fn render_level(
    directory: &Directory,
    level: usize,
    depth: usize,
    budget: &mut Budget,
    lines: &mut Vec<String>,
) {
    let indent = " ".repeat(4 * level);
    for (name, child) in &directory.entries {
        if budget.remaining == 0 {
            budget.omitted += child.as_ref().map_or(0, count_entries) + 1;
            continue;
        }
        budget.remaining -= 1;
        let Some(child) = child else {
            lines.push(format!("{indent}{}", one_line(name)));
            continue;
        };
        lines.push(format!("{indent}{}/", one_line(name)));
        if level + 1 >= depth {
            lines.push(format!("{indent}    ..."));
            continue;
        }
        render_level(child, level + 1, depth, budget, lines);
    }
}

// The names under a directory, the directory itself excluded.
fn count_entries(directory: &Directory) -> usize {
    directory
        .entries
        .values()
        .map(|child| 1 + child.as_ref().map_or(0, count_entries))
        .sum()
}

#[cfg(test)]
mod tests;
