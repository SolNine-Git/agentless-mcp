//! The module-level import cycles of a repository, in a fixed order, cut at the caller's limit.

use graph::{Scopes, SymbolGraph, import_cycles};
use kernel::Bounded;

use crate::bounds::within;
use crate::detached::answered_over;
use crate::ops::limits::MAX_LIMIT;
use crate::scan::RepoFacts;
use crate::{Error, IntArg, Noted, Reading, Services, Skipped};

/// A cycles request: how many cycles to list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CyclesRequest {
    /// The most cycles the answer lists. The count is always complete.
    pub limit: IntArg,
}

/// Every import cycle, cut at the limit, with what the resolver could not build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleReport {
    /// The cycles the limit kept, and how many there were.
    pub cycles: Bounded<CycleRow>,
    /// The import statements that named no file in this repository.
    pub unresolved_imports: u32,
    /// The unresolved import statements that named a module of this repository.
    pub unresolved_internal_imports: u32,
    /// The files the scan passed over.
    pub skipped: Vec<Skipped>,
}

/// One import cycle: the chain of files that closes it, and every file of its component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleRow {
    /// The chain, which closes on its first file.
    pub files: Vec<String>,
    /// Every file of the strongly connected component, sorted.
    pub members: Vec<String>,
}

/// Lists the import cycles of the repository that `reading` names. The scan's warnings come
/// with either outcome.
#[must_use]
pub fn cycles(
    services: &Services,
    reading: &Reading<'_>,
    request: &CyclesRequest,
) -> Noted<Result<CycleReport, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    RepoFacts::load(services, reading)
        .map(|loaded| loaded.map(|facts| answered_over(facts, |facts| report(facts, limit))))
}

fn report(facts: &RepoFacts, limit: usize) -> CycleReport {
    let scopes = Scopes::build(&facts.files);
    let imports = SymbolGraph::imports(&scopes);
    let rows = import_cycles(&imports)
        .into_iter()
        .map(|cycle| CycleRow {
            files: cycle.files.into_iter().map(str::to_owned).collect(),
            members: cycle.members.into_iter().map(str::to_owned).collect(),
        })
        .collect();
    CycleReport {
        cycles: Bounded::cut(rows, limit),
        unresolved_imports: imports.unresolved_imports(),
        unresolved_internal_imports: imports.unresolved_internal_imports(),
        skipped: facts.skipped.clone(),
    }
}

#[cfg(test)]
mod tests;
