//! POSIX path rules for import resolution: normalisation, stems, and the module-tail index over
//! the repository's files.

use std::collections::BTreeMap;

use text::{PosixPath, is_posix_part, posix_anchor};

const PACKAGE_ENTRY_STEMS: [&str; 2] = ["__init__", "index"];

/// Collapses `path` as POSIX normpath does: `.`, empty and `..` parts, and the leading slashes.
#[must_use]
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let anchor = posix_anchor(path.as_bytes());
    let mut kept: Vec<&str> = Vec::new();
    for part in path.split('/') {
        if !is_posix_part(part.as_bytes()) {
            continue;
        }
        let climbs = part == "..";
        if !climbs || (anchor.is_empty() && kept.is_empty()) || kept.last() == Some(&"..") {
            kept.push(part);
        } else {
            kept.pop();
        }
    }
    let joined = format!("{anchor}{}", kept.join("/"));
    if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

// The repository root normalises to "" rather than ".".
pub(crate) fn normalized(path: &str) -> String {
    let collapsed = normpath(path);
    if collapsed == "." {
        String::new()
    } else {
        collapsed
    }
}

pub(crate) fn stem(path: &str) -> String {
    PosixPath::parse(path).without_suffix().render()
}

// A package entry point also answers to the stem of its directory.
pub(crate) fn module_stems(path: &str) -> Vec<String> {
    let full = stem(path);
    let package = full
        .rsplit_once('/')
        .filter(|(_, last)| PACKAGE_ENTRY_STEMS.contains(last))
        .map(|(head, _)| head.to_owned());
    std::iter::once(full).chain(package).collect()
}

// A tail must hold a separator, so a match never falls inside one path component.
pub(crate) fn module_tail(module: &str) -> Option<String> {
    let tail = module.replace('.', "/");
    tail.contains('/').then_some(tail)
}

#[derive(Debug)]
pub(crate) struct PathIndex<'a> {
    paths: Vec<&'a str>,
    by_tail: BTreeMap<String, &'a str>,
}

impl<'a> PathIndex<'a> {
    pub(crate) fn build(paths: impl IntoIterator<Item = &'a str>) -> PathIndex<'a> {
        let mut sorted: Vec<&'a str> = paths.into_iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut by_tail = BTreeMap::new();
        for &path in &sorted {
            for stem in module_stems(path) {
                index_tails(&mut by_tail, &stem, path);
            }
        }
        PathIndex {
            paths: sorted,
            by_tail,
        }
    }

    pub(crate) fn get(&self, path: &str) -> Option<&'a str> {
        self.paths
            .binary_search(&path)
            .ok()
            .map(|found| self.paths[found])
    }

    // The paths that start with one prefix form one run of the sorted list, so the first path
    // at or after the prefix decides.
    pub(crate) fn has_prefix(&self, prefix: &str) -> bool {
        let at = self.paths.partition_point(|path| *path < prefix);
        self.paths
            .get(at)
            .is_some_and(|path| path.starts_with(prefix))
    }

    pub(crate) fn suffix_match(&self, module: &str) -> Option<&'a str> {
        let tail = module_tail(module)?;
        self.by_tail.get(&tail).copied()
    }
}

// Each tail keeps the path that is shortest in code points, then first in path order.
fn index_tails<'a>(by_tail: &mut BTreeMap<String, &'a str>, stem: &str, path: &'a str) {
    let segments: Vec<&str> = stem.split('/').collect();
    for start in 0..segments.len() {
        let held = by_tail.entry(segments[start..].join("/")).or_insert(path);
        if shorter(path, held) {
            *held = path;
        }
    }
}

fn shorter(path: &str, held: &str) -> bool {
    (path.chars().count(), path) < (held.chars().count(), held)
}
