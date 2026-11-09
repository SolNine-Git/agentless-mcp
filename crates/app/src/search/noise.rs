//! The noise classes: lock files, vendored files and minified files, whose matches a search counts
//! apart, and the cut that only a minified file's lines get.

use text::{posix_name, split_suffix};

use crate::scope::has_directory;

/// A file whose mean line is this long or longer, and whose longest line passes
/// [`MINIFIED_LONGEST_LINE_BYTES`], is minified.
pub const MINIFIED_MEAN_LINE_BYTES: usize = 300;
/// The longest line, in bytes, that a minified file by length must reach.
pub const MINIFIED_LONGEST_LINE_BYTES: usize = 2_000;
/// The characters that a cut line of a minified file keeps.
pub const MINIFIED_KEEP_CHARS: usize = 200;

// The names of the lock files that do not end in `.lock`.
const LOCK_NAMES: [&str; 6] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "go.sum",
    "packages.lock.json",
    "gradle.lockfile",
];
const LOCK_SUFFIX: &str = "lock";
const VENDOR_DIRECTORIES: [&str; 6] = [
    "vendor",
    "vendored",
    "third_party",
    "third-party",
    "node_modules",
    "bower_components",
];
// The suffixes that a minified bundle's name ends with after `.min`.
const MINIFIED_SUFFIXES: [&str; 3] = ["js", "mjs", "css"];
const MINIFIED_MARK: &str = ".min";

/// A class of files whose matches a search counts apart, unless the path names the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoiseClass {
    /// A package manager's lock file.
    Lock,
    /// A file below a vendored directory, such as `vendor/` or `node_modules/`.
    Vendored,
    /// A minified file, by its name or its line lengths.
    Minified,
}

impl NoiseClass {
    /// Returns the wire value, such as `lock`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            NoiseClass::Lock => "lock",
            NoiseClass::Vendored => "vendored",
            NoiseClass::Minified => "minified",
        }
    }
}

// A file gets the first class that holds: lock, then vendored, then minified.
pub(super) fn class_of(path: &str, minified: bool) -> Option<NoiseClass> {
    if is_lock(path) {
        Some(NoiseClass::Lock)
    } else if is_vendored(path) {
        Some(NoiseClass::Vendored)
    } else {
        minified.then_some(NoiseClass::Minified)
    }
}

fn is_lock(path: &str) -> bool {
    let name = posix_name(path);
    let (_, suffix) = split_suffix(name);
    suffix.strip_prefix('.') == Some(LOCK_SUFFIX) || LOCK_NAMES.contains(&name)
}

fn is_vendored(path: &str) -> bool {
    has_directory(path, &VENDOR_DIRECTORIES)
}

// A `.min.js` style name, or long lines on average with one very long line.
pub(super) fn is_minified(path: &str, lines: &[&str]) -> bool {
    let (stem, suffix) = split_suffix(posix_name(path));
    let named = stem.ends_with(MINIFIED_MARK)
        && (suffix.strip_prefix('.')).is_some_and(|suffix| MINIFIED_SUFFIXES.contains(&suffix));
    named || long_lines(lines)
}

fn long_lines(lines: &[&str]) -> bool {
    let longest = lines.iter().map(|line| line.len()).max().unwrap_or(0);
    let bytes: usize = lines.iter().map(|line| line.len()).sum();
    longest >= MINIFIED_LONGEST_LINE_BYTES
        && bytes >= MINIFIED_MEAN_LINE_BYTES.saturating_mul(lines.len())
}

// The first characters of a minified file's line, and how many the cut left out.
pub(super) fn cut(text: &str) -> (String, Option<usize>) {
    match text.char_indices().nth(MINIFIED_KEEP_CHARS) {
        Some((at, _)) => {
            let left = text[at..].chars().count();
            (text[..at].to_owned(), Some(left))
        }
        None => (text.to_owned(), None),
    }
}
