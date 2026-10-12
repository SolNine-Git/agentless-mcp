//! The directory that holds the cache of every repository.

use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use text::{CodePointString, is_space};

const APPLICATION_DIR: &str = "agentless-mcp";

/// The cache root, and the relative `XDG_CACHE_HOME` value that was ignored to find it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRoot {
    /// The directory that holds every repository's cache.
    pub path: PathBuf,
    /// The relative `XDG_CACHE_HOME` value, when one was ignored.
    pub ignored: Option<RelativeCacheHome>,
}

/// A relative `XDG_CACHE_HOME` value with its surrounding whitespace removed. It displays as the
/// warning about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelativeCacheHome(pub OsString);

impl fmt::Display for RelativeCacheHome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "XDG_CACHE_HOME={} is relative and was ignored; the XDG specification requires an \
             absolute path, and a relative one would put the cache inside whichever directory the \
             call was made from -- including the repository being analysed",
            CodePointString::from_utf8_escaped(self.0.as_bytes()).quoted()
        )
    }
}

/// `HOME` is unset and the user database names no home directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeUnknown;

impl fmt::Display for HomeUnknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Could not determine home directory.")
    }
}

impl std::error::Error for HomeUnknown {}

/// Returns `$XDG_CACHE_HOME/agentless-mcp` when that value is absolute after whitespace is
/// removed, else `$HOME/.cache/agentless-mcp`. It reads the variables through `env`.
///
/// # Errors
///
/// Returns [`HomeUnknown`] when the home directory is needed and unknown.
pub fn cache_root(env: impl Fn(&str) -> Option<OsString>) -> Result<CacheRoot, HomeUnknown> {
    let configured = env("XDG_CACHE_HOME").map(|value| strip(value.as_bytes()).to_vec());
    let (base, ignored) = match configured {
        Some(value) if value.starts_with(b"/") => (normalized(&value), None),
        Some(value) if !value.is_empty() => {
            let ignored = RelativeCacheHome(OsString::from_vec(value));
            (home_cache(&env)?, Some(ignored))
        }
        _ => (home_cache(&env)?, None),
    };
    Ok(CacheRoot {
        path: base.join(APPLICATION_DIR),
        ignored,
    })
}

fn home_cache(env: &impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, HomeUnknown> {
    let home = match env("HOME") {
        Some(home) => home,
        None => std::env::home_dir().ok_or(HomeUnknown)?.into_os_string(),
    };
    // The home path loses its trailing slashes, and an empty home is the root.
    let mut end = home.len();
    while end > 0 && home.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    let home: &[u8] = if end == 0 {
        b"/"
    } else {
        &home.as_bytes()[..end]
    };
    Ok(normalized(home).join(".cache"))
}

// The path text with repeated slashes and `.` parts removed; exactly two leading slashes stay.
fn normalized(bytes: &[u8]) -> PathBuf {
    let root: &[u8] = if bytes.starts_with(b"//") && !bytes.starts_with(b"///") {
        b"//"
    } else if bytes.starts_with(b"/") {
        b"/"
    } else {
        b""
    };
    let parts: Vec<&[u8]> = bytes
        .split(|&byte| byte == b'/')
        .filter(|part| !part.is_empty() && *part != b".")
        .collect();
    let mut text = root.to_vec();
    text.extend(parts.join(&b'/'));
    PathBuf::from(OsString::from_vec(text))
}

// Removes whitespace characters from both ends; a byte that is not UTF-8 is not whitespace.
fn strip(bytes: &[u8]) -> &[u8] {
    let leading = bytes.utf8_chunks().next().map_or(0, |chunk| {
        chunk.valid().len() - chunk.valid().trim_start_matches(is_space).len()
    });
    let rest = &bytes[leading..];
    let trailing = match rest.utf8_chunks().last() {
        Some(chunk) if chunk.invalid().is_empty() => {
            chunk.valid().len() - chunk.valid().trim_end_matches(is_space).len()
        }
        _ => 0,
    };
    &rest[..rest.len() - trailing]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::fixture;
    use std::collections::BTreeMap;

    // The variables, the expected path, and the expected warning.
    type Case = (BTreeMap<String, Vec<u8>>, Vec<u8>, Option<String>);

    #[test]
    fn cache_root_cases() {
        let all: Vec<Case> = fixture("cache", "cache_root");
        for (variables, expected_path, expected_warning) in all {
            let env = |name: &str| {
                variables
                    .get(name)
                    .map(|value| OsString::from_vec(value.clone()))
            };
            let root = cache_root(env).expect("every case sets HOME");
            let warning = root.ignored.as_ref().map(ToString::to_string);
            let found = (root.path.into_os_string().into_vec(), warning);
            assert_eq!(
                found,
                (expected_path, expected_warning),
                "variables {variables:?}"
            );
        }
    }

    #[test]
    fn home_unknown_displays_its_reason() {
        assert_eq!(
            HomeUnknown.to_string(),
            "Could not determine home directory."
        );
    }
}
