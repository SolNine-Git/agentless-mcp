//! The update of a kept graph from one scan to a later scan of the same paths: each part that the
//! changed files can change is changed in place, so that it equals a full build over the new scan.

use facts::Import;
use syntax::Language;

use crate::names::NameIndex;
use crate::rust_paths::is_manifest;
use crate::scan::FileFacts;
use crate::scopes::Scopes;

/// The kept parts of one scan's graph. A part that was not built is `None`, and stays so.
pub struct KeptParts<'p, 's> {
    pub index: Option<&'p mut NameIndex<'s>>,
    pub scopes: Option<&'p mut Scopes<'s>>,
}

/// What an update resolved again: the files whose every site it resolved, and the sites.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub files: usize,
    pub sites: usize,
}

/// Why an update hands a scan to a full build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebuild {
    /// A file was added, deleted or renamed, or it started or stopped parsing.
    Paths,
    /// The package facts of a Cargo manifest changed.
    Manifest,
    /// The `use` declarations or the `mod` items of a Rust file changed.
    RustModules,
}

/// Carry `parts` from the scan `before` to the scan `after`. A file whose facts did not change
/// keeps its facts' address in `after`, and every other file counts as changed.
///
/// # Errors
///
/// Returns the [`Rebuild`] case of a scan that the update leaves to a full build. The parts are
/// then partly updated, and only a full build may follow.
pub fn update<'s>(
    parts: KeptParts<'_, 's>,
    before: &'s [&'s FileFacts],
    after: &'s [&'s FileFacts],
) -> Result<Resolved, Rebuild> {
    let changed = changed(before, after)?;
    if let Some(index) = parts.index {
        index.update(after, &changed);
    }
    if let Some(scopes) = parts.scopes {
        scopes.update(after, &changed);
    }
    Ok(Resolved::default())
}

// The positions of the changed files, or the case that needs a full build.
fn changed(before: &[&FileFacts], after: &[&FileFacts]) -> Result<Vec<usize>, Rebuild> {
    let same_path = |(old, new): (&&FileFacts, &&FileFacts)| {
        old.path == new.path && old.language == new.language
    };
    if before.len() != after.len() || !before.iter().zip(after).all(same_path) {
        return Err(Rebuild::Paths);
    }
    let pairs = before.iter().zip(after).enumerate();
    let changed: Vec<usize> = (pairs)
        .filter(|(_, (old, new))| !std::ptr::eq(**old, **new))
        .map(|(at, _)| at)
        .collect();
    for &at in &changed {
        rebuilds(before, after[at])?;
        let (old, new) = (before[at], after[at]);
        if is_manifest(new) && old.facts.manifest != new.facts.manifest {
            return Err(Rebuild::Manifest);
        }
        if new.language == Language::Rust && !same_modules(old, new) {
            return Err(Rebuild::RustModules);
        }
    }
    Ok(changed)
}

// A repeated path answers with its last file in the scopes, so its files go to a full build.
fn rebuilds(files: &[&FileFacts], file: &FileFacts) -> Result<(), Rebuild> {
    let holding = files.iter().filter(|held| held.path == file.path).count();
    if holding > 1 {
        return Err(Rebuild::Paths);
    }
    Ok(())
}

// The export passes read every Rust file's `use` declarations, but no import's line.
fn same_modules(old: &FileFacts, new: &FileFacts) -> bool {
    let (left, right) = (&old.facts.imports, &new.facts.imports);
    old.facts.modules == new.facts.modules
        && left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| binds_alike(left, right))
}

fn binds_alike(left: &Import, right: &Import) -> bool {
    let Import {
        module,
        names,
        local_names,
        is_relative,
        relative_level,
        line: _,
        binds_all,
        alias,
        inline_depth,
        runs,
    } = left;
    (module, names, local_names, is_relative, relative_level)
        == (
            &right.module,
            &right.names,
            &right.local_names,
            &right.is_relative,
            &right.relative_level,
        )
        && (binds_all, alias, inline_depth, runs)
            == (
                &right.binds_all,
                &right.alias,
                &right.inline_depth,
                &right.runs,
            )
}
