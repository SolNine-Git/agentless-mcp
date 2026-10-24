//! The rules of a pure POSIX path: its anchor, its parts, its name and the last suffix of a name.
//! Nothing here reads the file system.

/// A POSIX path text split into its anchor and its parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PosixPath<'s> {
    anchor: &'static str,
    parts: Vec<&'s str>,
}

impl<'s> PosixPath<'s> {
    /// Splits `text` into the anchor of [`posix_anchor`] and the parts that [`is_posix_part`] keeps.
    #[must_use]
    pub fn parse(text: &'s str) -> PosixPath<'s> {
        let anchor = posix_anchor(text.as_bytes());
        let parts = text
            .split('/')
            .filter(|part| is_posix_part(part.as_bytes()))
            .collect();
        PosixPath { anchor, parts }
    }

    /// Returns the path without its last part. The anchor, or the empty path ".", is its own parent.
    #[must_use]
    pub fn parent(&self) -> PosixPath<'s> {
        let kept = self.parts.len().saturating_sub(1);
        PosixPath {
            anchor: self.anchor,
            parts: self.parts[..kept].to_vec(),
        }
    }

    /// Returns the number of parts, with the anchor counted as one part.
    #[must_use]
    pub fn part_count(&self) -> usize {
        self.parts.len() + usize::from(!self.anchor.is_empty())
    }

    /// Returns the text of `tail` below this path. An absolute tail replaces the path.
    #[must_use]
    pub fn join(&self, tail: &str) -> String {
        if tail.starts_with('/') {
            return tail.to_owned();
        }
        format!("{}/{tail}", self.render())
    }

    /// Returns the path with the last suffix of its last part removed, by [`split_suffix`].
    #[must_use]
    pub fn without_suffix(&self) -> PosixPath<'s> {
        let mut parts = self.parts.clone();
        if let Some(last) = parts.last_mut() {
            *last = split_suffix(last).0;
        }
        PosixPath {
            anchor: self.anchor,
            parts,
        }
    }

    /// Returns the path text: the anchor and the parts joined by "/", or "." for an empty path.
    #[must_use]
    pub fn render(&self) -> String {
        let joined = self.parts.join("/");
        if self.anchor.is_empty() && joined.is_empty() {
            return ".".to_owned();
        }
        format!("{}{joined}", self.anchor)
    }
}

/// Returns the anchor of `path`: "//" for exactly two leading slashes, "/" for one or for three or
/// more, and "" for none.
#[must_use]
pub fn posix_anchor(path: &[u8]) -> &'static str {
    if path.starts_with(b"//") && !path.starts_with(b"///") {
        "//"
    } else if path.starts_with(b"/") {
        "/"
    } else {
        ""
    }
}

/// Returns true for a component between slashes that a path keeps: one that is neither empty nor
/// ".". A ".." part is kept.
#[must_use]
pub fn is_posix_part(part: &[u8]) -> bool {
    !part.is_empty() && part != b"."
}

/// Returns the last part of `path`, or an empty text when the path has no part.
#[must_use]
pub fn posix_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| is_posix_part(part.as_bytes()))
        .unwrap_or_default()
}

/// Splits `name` at the dot that starts its last suffix: the last dot, when it is neither the
/// first nor the last character. The suffix keeps its dot, and is empty when there is none.
#[must_use]
pub fn split_suffix(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name.split_at(dot),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::PosixPath;

    #[test]
    fn a_pure_path_keeps_a_double_slash_root_and_never_empties() {
        let texts = ["a//b/./c", "", "./", "/", "//", "///a", "//a/b/", "a/../b"];
        let posix: Vec<String> = texts
            .iter()
            .map(|text| PosixPath::parse(text).render())
            .collect();
        assert_eq!(
            posix,
            ["a/b/c", ".", ".", "/", "//", "/a", "//a/b", "a/../b"]
        );
    }
}
