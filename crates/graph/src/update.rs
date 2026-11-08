//! The update of a kept graph from one scan to a later scan of the same paths: each part that the
//! changed files can change is changed in place, so that it equals a full build over the new scan.

use facts::Import;
use syntax::Language;

use crate::affected::Affected;
use crate::aliases::Aliases;
use crate::ambiguous::AmbiguousDegrees;
use crate::cycles::{Cycle, import_cycles};
use crate::names::NameIndex;
use crate::resolver::Resolver;
use crate::rust_paths::is_manifest;
use crate::scan::FileFacts;
use crate::scopes::{ImportScope, Scopes};
use crate::symbols::{Nodes, SymbolGraph};

/// The kept parts of one scan's graph. A part that was not built is `None`, and stays so.
pub struct KeptParts<'p, 's> {
    pub index: Option<&'p mut NameIndex<'s>>,
    pub scopes: Option<&'p mut Scopes<'s>>,
    pub graph: Option<&'p mut SymbolGraph<'s>>,
    pub degrees: Option<&'p mut AmbiguousDegrees>,
    pub aliases: Option<&'p mut Aliases<'s>>,
    pub imports: Option<&'p mut SymbolGraph<'s>>,
    pub cycles: Option<&'p mut Vec<Cycle<'s>>>,
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
    /// A part was kept without a part that it is built from.
    Unbuilt,
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
    if !parts.built_from_their_parts() {
        return Err(Rebuild::Unbuilt);
    }
    let KeptParts {
        index,
        scopes,
        graph,
        degrees,
        aliases,
        imports,
        cycles,
    } = parts;
    let index = index.map(|index| {
        index.update(after, &changed);
        &*index
    });
    let scopes = scopes.map(|scopes| {
        let rebound = scopes.update(after, &changed);
        (&*scopes, rebound)
    });
    let (Some(index), Some((scopes, rebound))) = (index, scopes) else {
        return Ok(Resolved::default());
    };
    let restated = (rebound.iter()).any(|(at, old)| old.statements != scopes.at(*at).statements);
    if let Some(imports) = imports.filter(|_| restated) {
        *imports = SymbolGraph::imports(scopes);
        if let Some(cycles) = cycles {
            *cycles = import_cycles(imports);
        }
    }
    if graph.is_none() && aliases.is_none() {
        return Ok(Resolved::default());
    }
    let affected = Affected::of(before, after, &changed, Some((scopes, &rebound)));
    let resolver = Resolver::new(index, scopes);
    let mut work = Resolved {
        files: affected.whole.len(),
        sites: 0,
    };
    if let Some(graph) = graph {
        let linked = Linked {
            resolver: &resolver,
            before,
            after,
            affected: &affected,
        };
        work.sites = linked.relink(graph, degrees, &rebound);
    }
    if let Some(aliases) = aliases {
        aliases.update(&resolver, after, &affected);
    }
    Ok(work)
}

impl KeptParts<'_, '_> {
    // Each part kept with the parts it is built from: the graph and the aliases resolve over the
    // index and the scopes, the degrees count the graph, and the cycles walk the import graph.
    fn built_from_their_parts(&self) -> bool {
        let resolving = self.index.is_some() && self.scopes.is_some();
        (resolving || (self.graph.is_none() && self.aliases.is_none()))
            && (self.degrees.is_none() || self.graph.is_some())
            && (self.imports.is_none() || self.scopes.is_some())
            && (self.cycles.is_none() || self.imports.is_some())
    }
}

// What the graph of an update links again over.
struct Linked<'l, 's> {
    resolver: &'l Resolver<'l, 's>,
    before: &'l [&'s FileFacts],
    after: &'s [&'s FileFacts],
    affected: &'l Affected<'s>,
}

impl<'s> Linked<'_, 's> {
    // Links the affected sites of `graph` again, and the degrees of the names it changes. Returns
    // how many sites it resolved.
    fn relink(
        &self,
        graph: &mut SymbolGraph<'s>,
        degrees: Option<&mut AmbiguousDegrees>,
        rebound: &[(usize, ImportScope<'s>)],
    ) -> usize {
        let nodes = Nodes::new(self.after, true);
        let relinked = graph.relinked(&nodes, self.resolver, self.before, self.affected);
        let sites = relinked.sites();
        let names = relinked.names();
        let tally = degrees.map(|degrees| (graph.degrees_of(names.iter().copied()), degrees));
        graph.relink(relinked, self.before, self.affected);
        graph.recount(self.resolver.scopes(), rebound);
        if let Some((old, degrees)) = tally {
            degrees.replace(&old, &graph.degrees_of(names.iter().copied()));
        }
        sites
    }
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
