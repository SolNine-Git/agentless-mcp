//! The tree side of `callee_overlap`: one scan of the working tree under the walk budget, its
//! resolved symbol graph, and the counted callees and spelled names of each function it reads.

use std::path::Path;
use std::sync::Arc;

use checks::{Callee, CalleeTree, TreeFunction};
use facts::{Ref, StableId, Symbol, SymbolKind};
use graph::{FileFacts, Relation, SymbolGraph};
use kernel::Deadline;
use store::FactSource;
use syntax::Language;

use crate::detached::answered_over;
use crate::scan::RepoFacts;
use crate::symbol_graph::with_resolved;
use crate::{Error, Noted, Services, in_metric_scope};

// Returns true when callee_overlap reads `path`: a tier-1 source file outside test and fixture
// paths, in a language whose table holds a callee stoplist.
pub(crate) fn in_callee_scope(path: &str) -> bool {
    in_metric_scope(path) && stoplist(path).is_some()
}

fn stoplist(path: &str) -> Option<&'static [&'static str]> {
    Language::from_path(path)?.callee_stoplist()
}

// A stop of the scan or the build gives the stopped tree: a partial edge set makes an existing
// function's set smaller and can make a false row.
pub(crate) fn callee_tree(services: &Services, root: &Path) -> Noted<Result<CalleeTree, Error>> {
    let budget = services.walk_budget;
    let deadline = Deadline::new(budget.as_secs_f64(), 0.0, Arc::clone(&services.clock));
    let source = FactSource::parsing();
    let scanned = RepoFacts::load_at(services, root, &source, Some(&deadline));
    scanned.map(|scanned| {
        let built = scanned.and_then(|repo| answered_over(repo, |repo| resolved(repo, &deadline)));
        match built {
            Ok(functions) => Ok(CalleeTree::Built(functions)),
            Err(Error::Kind(kernel::Error::CallStopped(_))) => Ok(CalleeTree::Stopped {
                seconds: budget.as_secs(),
            }),
            Err(error) => Err(error),
        }
    })
}

fn resolved(repo: &RepoFacts, deadline: &Deadline) -> Result<Vec<TreeFunction>, Error> {
    with_resolved(&repo.files, Some(deadline), |_, graph| {
        Ok(functions(&repo.files, graph))
    })
}

fn functions(files: &[FileFacts], graph: &SymbolGraph<'_>) -> Vec<TreeFunction> {
    files
        .iter()
        .filter(|file| in_callee_scope(&file.path))
        .flat_map(|file| file_functions(file, graph))
        .collect()
}

fn file_functions(file: &FileFacts, graph: &SymbolGraph<'_>) -> Vec<TreeFunction> {
    let stoplist = stoplist(&file.path).expect("a file in callee scope has a stoplist");
    file.facts
        .symbols
        .iter()
        .filter(|symbol| is_function(symbol) && !symbol.marks.protocol && !symbol.marks.test)
        .map(|symbol| {
            let id = StableId::of(file.language, &file.path, symbol).to_string();
            TreeFunction {
                path: file.path.clone(),
                name: symbol.name.clone(),
                qualname: symbol.qualname(),
                line: symbol.line,
                name_line: symbol.name_line,
                callees: callees(graph, &id, stoplist),
                spelled: spelled(&file.facts.refs, symbol),
            }
        })
        .collect()
}

fn is_function(symbol: &Symbol) -> bool {
    matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
}

// Each function or method that a reference edge of `node` reaches at a binding tier, once, and
// never test code, which only test code calls. The first edge of a target is its strongest.
fn callees(graph: &SymbolGraph<'_>, node: &str, stoplist: &[&str]) -> Vec<Callee> {
    let mut callees: Vec<Callee> = Vec::new();
    for edge in graph.outgoing(node) {
        if edge.relation() != Relation::References || !edge.tier().binds() {
            continue;
        }
        let target = edge.target();
        if callees.last().is_some_and(|last| last.id == target.node()) {
            continue;
        }
        let Some(definition) = graph.definitions().get(target.node()) else {
            continue;
        };
        let symbol = definition.symbol();
        let counted = is_function(symbol) && !symbol.marks.test;
        if counted && stoplist.binary_search(&symbol.name.as_str()).is_err() {
            callees.push(Callee {
                id: target.node().to_owned(),
                name: symbol.name.clone(),
                qualname: target.label().to_owned(),
                tier: edge.tier().as_str(),
            });
        }
    }
    callees
}

// Each name that an identifier on the symbol's lines spells, sorted, each once. The references
// are in document order, so the symbol's first one is found by its line.
fn spelled(refs: &[Ref], symbol: &Symbol) -> Vec<String> {
    let first = refs.partition_point(|reference| reference.line < symbol.line);
    let mut names: Vec<String> = refs[first..]
        .iter()
        .take_while(|reference| reference.line <= symbol.end_line)
        .map(|reference| reference.name.clone())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}
