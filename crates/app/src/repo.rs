//! The repository an operation reads: its resolved root and its config.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::{Error, Services};

/// A repository root, resolved and checked, with its project config.
#[derive(Clone, Debug)]
pub struct Repo {
    /// The resolved root directory.
    pub root: PathBuf,
    /// The checked `.agentless-mcp.json` values and their warnings.
    pub config: io::ProjectConfig,
}

impl Repo {
    /// Resolves `root` against the start-up directory, checks that it is a directory, and loads
    /// its config.
    ///
    /// # Errors
    ///
    /// Returns a repository-resolution error when the root is not a directory or not a UTF-8 path,
    /// or when it is relative and the start-up directory is unknown.
    pub fn load(services: &Services, root: &Path) -> Result<Repo, Error> {
        let root = root_directory(&below_cwd(services, root)?)?;
        let config = io::load_config(&root);
        Ok(Repo { root, config })
    }
}

// An absolute path stands alone. A relative one needs the start-up directory.
fn below_cwd(services: &Services, path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(start_dir(services)?.join(path))
}

fn start_dir(services: &Services) -> Result<&Path, Error> {
    services
        .cwd
        .as_deref()
        .map_err(|unread| kernel::Error::RepoResolution(unread.to_string()).into())
}

/// Resolves the repository a command is about: `repo` when given, else the git root that encloses
/// the start-up directory, else that directory. Loads its config.
///
/// # Errors
///
/// Returns a repository-resolution error when the root is not a directory or not a UTF-8 path.
pub fn resolve(services: &Services, repo: Option<&Path>) -> Result<Repo, Error> {
    let raw = if let Some(path) = repo {
        expand_user(path, services.home.as_deref())
    } else {
        let cwd = start_dir(services)?;
        services.git.root(cwd).unwrap_or_else(|| cwd.to_path_buf())
    };
    let root = root_directory(&below_cwd(services, &raw)?)?;
    let config = io::load_config(&root);
    Ok(Repo { root, config })
}

fn root_directory(raw: &Path) -> Result<PathBuf, Error> {
    let root =
        io::resolve(raw).map_err(|error| kernel::Error::OperationFailed(io::strerror(&error)))?;
    if !root.is_dir() {
        let message = format!("not a directory: {}", root.display());
        return Err(kernel::Error::RepoResolution(message).into());
    }
    if root.to_str().is_none() {
        let message = format!("not a UTF-8 path: {}", root.display());
        return Err(kernel::Error::RepoResolution(message).into());
    }
    Ok(root)
}

// A leading "~" or "~/" becomes the home directory. "~user" stays as it is.
fn expand_user(path: &Path, home: Option<&OsStr>) -> PathBuf {
    let bytes = path.as_os_str().as_bytes();
    let rest = match bytes.strip_prefix(b"~") {
        Some(rest) if rest.is_empty() || rest.starts_with(b"/") => rest,
        _ => return path.to_path_buf(),
    };
    let home = match home {
        Some(home) => home.to_os_string(),
        None => match std::env::home_dir() {
            Some(home) => home.into_os_string(),
            None => return path.to_path_buf(),
        },
    };
    let mut expanded = home.as_bytes().to_vec();
    while expanded.last() == Some(&b'/') {
        expanded.pop();
    }
    expanded.extend_from_slice(rest);
    if expanded.is_empty() {
        expanded.push(b'/');
    }
    PathBuf::from(OsStr::from_bytes(&expanded))
}

#[cfg(test)]
mod tests;
