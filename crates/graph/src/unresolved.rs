//! The two counts an import result is read against: every unresolved statement, and those that
//! named something this repository holds.

use std::collections::BTreeSet;

use crate::scopes::{ResolvedImport, Scopes};

pub(crate) fn unresolved_imports(scopes: &Scopes<'_>) -> u32 {
    count(scopes, |_| true)
}

// A relative import can name nothing but this repository; an absolute one does when it leads
// with a directory name or a file stem here.
pub(crate) fn unresolved_internal_imports(scopes: &Scopes<'_>) -> u32 {
    let segments = repository_segments(scopes.files().iter().map(|file| file.path.as_str()));
    count(scopes, |row| {
        row.relative || names_this_repository(&row.module, &segments)
    })
}

fn count(scopes: &Scopes<'_>, internal: impl Fn(&ResolvedImport<'_>) -> bool) -> u32 {
    let rows = scopes
        .iter()
        .flat_map(|(_, scope)| &scope.statements)
        .filter(|row| row.resolved.is_none() && internal(row))
        .count();
    u32::try_from(rows).expect("a scan holds fewer than 2^32 import statements")
}

pub(crate) fn repository_segments<'p>(paths: impl Iterator<Item = &'p str>) -> BTreeSet<&'p str> {
    let mut segments = BTreeSet::new();
    for path in paths {
        let mut parts: Vec<&str> = path.split('/').collect();
        let last = parts.pop().unwrap_or_default();
        segments.extend(parts);
        let stem = last.rsplit_once('.').map_or("", |(stem, _)| stem);
        segments.insert(if stem.is_empty() { last } else { stem });
    }
    segments
}

pub(crate) fn names_this_repository(module: &str, segments: &BTreeSet<&str>) -> bool {
    let text = text::strip(module);
    if text.is_empty() {
        return false;
    }
    let spelled = text.replace('\\', "/").replace("::", "/").replace('.', "/");
    let lead = spelled.split('/').next().unwrap_or_default();
    !lead.is_empty() && segments.contains(lead)
}
