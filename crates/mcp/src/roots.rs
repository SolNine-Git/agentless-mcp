//! The repositories a server may serve: the `--root` flags, then each `--roots-from` file as it
//! reads now, and the selection of one call's repository among them.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{Error as OsError, Result as OsResult};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use app::Services;
use prompts::messages::{
    REPO_REFUSED_NOT_ALLOWED, ROOTS_FILE_HINT, ROOTS_FILE_UNREADABLE, SERVER_NO_ROOTS,
    SERVER_ROOT_REQUIRED,
};
use text::PosixPath;

use crate::failure::Failure;

const BOM: &[u8] = b"\xef\xbb\xbf";

/// A `--roots-from` file: one repository path per line, read again when it changes.
pub struct RootsFile {
    path: PathBuf,
    anchor: Anchor,
    state: Mutex<Listed>,
}

// The roots the file held when last read, and the (mtime_ns, size) it was read at.
struct Listed {
    roots: Vec<PathBuf>,
    key: (i128, u64),
}

/// Why a roots file could not be read at start-up. It displays as the start-up refusal.
#[derive(Debug)]
pub enum RootsFileError {
    /// The stat or the read failed.
    Unreadable {
        /// The path as the command line gave it.
        raw: String,
        /// The text of the operating system's error.
        error: String,
    },
    /// The file is not UTF-8 text.
    NotUtf8 {
        /// The path as the command line gave it.
        raw: String,
        /// The text of the decode error.
        error: String,
    },
}

impl fmt::Display for RootsFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { raw, error } => write!(f, "cannot read roots file {raw}: {error}"),
            Self::NotUtf8 { raw, error } => {
                write!(f, "roots file {raw} is not valid UTF-8: {error}")
            }
        }
    }
}

impl std::error::Error for RootsFileError {}

// Why a read of a roots file failed: the OS error, the decode error, or a line that names no path.
enum ReadError {
    Os(String),
    Decode(String),
    Line(String),
}

// The directories that a written root resolves against: the home of a leading `~`, and the
// start-up directory of a relative path.
#[derive(Clone, Debug)]
pub(crate) struct Anchor {
    home: Option<OsString>,
    cwd: Option<PathBuf>,
}

impl Anchor {
    pub(crate) fn of(services: &Services) -> Anchor {
        Anchor {
            home: services.home.clone(),
            cwd: services.cwd.as_ref().ok().cloned(),
        }
    }
}

impl RootsFile {
    /// Reads the roots file at `path`. A line resolves against the home and the start-up
    /// directory of `services`.
    ///
    /// # Errors
    ///
    /// Returns [`RootsFileError`] when the file cannot be read or is not UTF-8 text.
    pub fn read(path: &Path, services: &Services) -> Result<RootsFile, RootsFileError> {
        let raw = path.to_string_lossy().into_owned();
        let anchor = Anchor::of(services);
        let read = listed(path, &anchor).map_err(|error| match error {
            ReadError::Os(error) | ReadError::Line(error) => RootsFileError::Unreadable {
                raw: raw.clone(),
                error,
            },
            ReadError::Decode(error) => RootsFileError::NotUtf8 {
                raw: raw.clone(),
                error,
            },
        })?;
        Ok(RootsFile {
            path: path.to_path_buf(),
            anchor,
            state: Mutex::new(read),
        })
    }

    /// Returns the path as the command line gave it.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    // The roots as of now: the same list while (mtime_ns, size) holds, else the file read again.
    fn current(&self) -> Result<Vec<PathBuf>, Failure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = stat_key(&self.path).map_err(|error| self.unreadable(&error))?;
        if key == state.key {
            return Ok(state.roots.clone());
        }
        let read = listed(&self.path, &self.anchor).map_err(|error| match error {
            ReadError::Os(error) | ReadError::Decode(error) => self.refusal(&error),
            ReadError::Line(error) => Failure::Unplanned(error),
        })?;
        *state = read;
        Ok(state.roots.clone())
    }

    fn shown(&self) -> String {
        PosixPath::parse(&self.path.to_string_lossy()).render()
    }

    fn unreadable(&self, error: &OsError) -> Failure {
        self.refusal(&io::os_error_text(error, &self.path))
    }

    fn refusal(&self, error: &str) -> Failure {
        Failure::Refused(ROOTS_FILE_UNREADABLE.render((&self.shown(), error)))
    }
}

fn stat_key(path: &Path) -> OsResult<(i128, u64)> {
    let metadata = fs::metadata(path)?;
    let nanos = i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
    Ok((nanos, metadata.size()))
}

// The stat comes before the read, so a write between the two costs one more read, never a stale
// list.
fn listed(path: &Path, anchor: &Anchor) -> Result<Listed, ReadError> {
    let os = |error: OsError| ReadError::Os(io::os_error_text(&error, path));
    let key = stat_key(path).map_err(os)?;
    let bytes = fs::read(path).map_err(os)?;
    let body = bytes.strip_prefix(BOM).unwrap_or(&bytes);
    let text = io::decode_text(body.to_vec())
        .map_err(|error| ReadError::Decode(io::codec_text(body, error)))?;
    let roots = root_lines(&text)
        .into_iter()
        .map(|line| resolved(line, anchor))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ReadError::Line)?;
    Ok(Listed { roots, key })
}

// The lines that name a root: each stripped, with blank and `#` comment lines left out.
fn root_lines(text: &str) -> Vec<&str> {
    text::splitlines(text, false)
        .into_iter()
        .map(text::strip)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

// The absolute form of a root as written: a leading `~` expanded, then resolved.
fn resolved(root: &str, anchor: &Anchor) -> Result<PathBuf, String> {
    let expanded = app::expand_user(Path::new(root), anchor.home.as_deref());
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        let cwd = anchor
            .cwd
            .as_ref()
            .ok_or("the start-up directory is unknown")?;
        cwd.join(expanded)
    };
    io::resolve(&absolute).map_err(|error| error.to_string())
}

// The allowlist: the roots of the flags, then those of each roots file.
pub(crate) struct Allowlist {
    flags: Vec<PathBuf>,
    files: Vec<RootsFile>,
}

impl Allowlist {
    pub(crate) fn new(flags: Vec<PathBuf>, files: Vec<RootsFile>) -> Allowlist {
        Allowlist { flags, files }
    }

    // The flags, then each file's roots as of now, each root once, in first-seen order.
    pub(crate) fn current(&self) -> Result<Vec<PathBuf>, Failure> {
        let mut roots = self.flags.clone();
        for file in &self.files {
            roots.extend(file.current()?);
        }
        let mut seen = Vec::new();
        roots.retain(|root| {
            let fresh = !seen.contains(root);
            if fresh {
                seen.push(root.clone());
            }
            fresh
        });
        Ok(roots)
    }

    // The roots-file hint that a refusal ends with, or nothing when no roots file is set.
    pub(crate) fn hint(&self) -> Option<String> {
        (!self.files.is_empty())
            .then(|| ROOTS_FILE_HINT.render(&joined(&self.files, RootsFile::shown)))
    }
}

// What a call asks for and what the server knows when it selects the repository.
pub(crate) struct Selection<'a> {
    pub(crate) allowed: &'a [PathBuf],
    pub(crate) requested: Option<&'a str>,
    pub(crate) launch: Option<&'a Path>,
    pub(crate) anchor: &'a Anchor,
    pub(crate) hint: Option<&'a str>,
}

// The root a call reads: the one it names, which must be allowed, or the only root, or the one
// root that holds the launch directory.
pub(crate) fn selected(selection: &Selection<'_>) -> Result<PathBuf, Failure> {
    let refused = |message: String| Failure::Refused(hinted(message, selection.hint));
    if selection.allowed.is_empty() {
        return Err(refused(SERVER_NO_ROOTS.to_owned()));
    }
    let requested = selection
        .requested
        .filter(|root| !text::strip(root).is_empty());
    let Some(requested) = requested else {
        let listing = joined(selection.allowed, shown_path);
        return sole_root(selection.allowed, selection.launch)
            .ok_or_else(|| refused(SERVER_ROOT_REQUIRED.render(&listing)));
    };
    let resolved = resolved(requested, selection.anchor).map_err(Failure::Unplanned)?;
    if selection.allowed.contains(&resolved) {
        return Ok(resolved);
    }
    let mut sorted = selection.allowed.to_vec();
    sorted.sort_by_key(|root| components(root));
    sorted.dedup();
    Err(refused(
        REPO_REFUSED_NOT_ALLOWED.render(&joined(&sorted, shown_path)),
    ))
}

// The only root, or the one root that holds the launch directory.
fn sole_root(allowed: &[PathBuf], launch: Option<&Path>) -> Option<PathBuf> {
    if let [only] = allowed {
        return Some(only.clone());
    }
    let launch = launch?;
    let mut holding = allowed.iter().filter(|root| launch.starts_with(root));
    match (holding.next(), holding.next()) {
        (Some(root), None) => Some(root.clone()),
        _ => None,
    }
}

fn hinted(message: String, hint: Option<&str>) -> String {
    match hint {
        Some(hint) => format!("{message} {hint}"),
        None => message,
    }
}

fn shown_path(path: &impl AsRef<Path>) -> String {
    path.as_ref().display().to_string()
}

fn joined<T>(items: &[T], show: impl Fn(&T) -> String) -> String {
    let shown: Vec<String> = items.iter().map(show).collect();
    shown.join(", ")
}

// A path's parts split at each "/", the order in which the reference sorts paths.
fn components(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy();
    text.split('/').map(str::to_owned).collect()
}
