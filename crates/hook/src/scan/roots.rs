//! The repository roots that a scan covers: those of a shell command, less the ignored ones.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use app::Services;

use crate::repository::{self, NoRoot};
use crate::search::write::source_paths;

const MAX_ROOTS: usize = 6;
const HOME: &str = "~";

// The roots, and the first reason that git could not name the root of a folder.
pub(crate) struct Found {
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) error: Option<String>,
}

// The roots of the working folder and of each source path that the command names, at most six.
pub(crate) fn of_command(
    services: &Services,
    home: Option<&Path>,
    cwd: &Path,
    command: &str,
) -> Found {
    let paths = source_paths(command).into_iter();
    let starts = std::iter::once(Some(cwd.to_path_buf()))
        .chain(paths.map(|token| source_folder(home, cwd, token)));
    let mut found = Found {
        roots: Vec::new(),
        error: None,
    };
    for start in starts.flatten() {
        match repository::root_of(services, &start) {
            Ok(root) if !found.roots.contains(&root) => found.roots.push(root),
            Ok(_) | Err(NoRoot::Outside) => {}
            Err(no_root) => found.error = found.error.take().or(no_root.cause()),
        }
        if found.roots.len() == MAX_ROOTS {
            break;
        }
    }
    found
}

// The nearest existing folder at or above the folder of the path that a command names.
fn source_folder(home: Option<&Path>, cwd: &Path, token: &str) -> Option<PathBuf> {
    let path = cwd.join(expanded(home, Path::new(token)));
    let mut folder = path.parent()?.to_path_buf();
    while !folder.is_dir() {
        folder = folder.parent()?.to_path_buf();
    }
    Some(folder)
}

// The path with a leading `~` read as the home folder, as a shell reads it.
pub(crate) fn expanded(home: Option<&Path>, path: &Path) -> PathBuf {
    match (home, path.strip_prefix(HOME)) {
        (Some(home), Ok(rest)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

// The roots that the setting lists: absolute paths, `:`-separated, `~` expanded, resolved.
pub(crate) fn ignored(setting: Option<&OsStr>, home: Option<&Path>) -> Vec<PathBuf> {
    let listed = setting.map(std::env::split_paths).into_iter().flatten();
    (listed.filter(|entry| !entry.as_os_str().is_empty()))
        .map(|entry| expanded(home, &entry))
        .filter(|entry| entry.is_absolute())
        .map(|entry| io::resolve(&entry).unwrap_or(entry))
        .collect()
}

#[cfg(test)]
mod tests;
