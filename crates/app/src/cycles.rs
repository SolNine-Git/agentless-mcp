//! The module-level import cycles of a repository, in a fixed order, cut at the caller's limit.

use graph::{Cycle, Scopes, SymbolGraph, import_cycles};
use kernel::Bounded;

use crate::bounds::within;
use crate::ops::limits::MAX_LIMIT;
use crate::scan::{RepoFacts, over_scan};
use crate::symbol_graph::{CallGraph, CallScan};
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
    over_scan(services, reading, |facts| Ok(report(facts, limit)))
}

// The cycles as `cycles` lists them, over the graph that one trace call reads for all its cards.
// The warnings of the call's scan come first, as the scan of `cycles` gives them.
pub(crate) fn cycles_in(
    request: &CyclesRequest,
    graph: &CallGraph<'_>,
    scan: &CallScan,
) -> Noted<Result<Bounded<CycleRow>, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    let answered = graph.over(move |graph| {
        let imports = SymbolGraph::imports(graph.scopes());
        Ok(Bounded::cut(rows(&import_cycles(&imports)), limit))
    });
    let mut diagnostics = scan.warnings.clone();
    diagnostics.extend(answered.diagnostics);
    Noted {
        value: answered.value,
        diagnostics,
    }
}

fn report(facts: &RepoFacts, limit: usize) -> CycleReport {
    let scopes = Scopes::build(&facts.files);
    let imports = SymbolGraph::imports(&scopes);
    CycleReport {
        cycles: Bounded::cut(rows(&import_cycles(&imports)), limit),
        unresolved_imports: imports.unresolved_imports(),
        unresolved_internal_imports: imports.unresolved_internal_imports(),
        skipped: facts.skipped.clone(),
    }
}

fn rows(cycles: &[Cycle<'_>]) -> Vec<CycleRow> {
    let owned = |paths: &[&str]| paths.iter().map(|path| (*path).to_owned()).collect();
    (cycles.iter())
        .map(|cycle| CycleRow {
            files: owned(&cycle.files),
            members: owned(&cycle.members),
        })
        .collect()
}

#[cfg(test)]
mod tests;
