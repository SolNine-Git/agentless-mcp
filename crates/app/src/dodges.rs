//! The tree side of `cycle_dodge`: the import facts of the tree after the change (the shared scan)
//! and of the tree before it, the import graph of each, and the load-time chains between files.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use checks::{
    BudgetSpent, ChainHop, DodgeTree, ImportTarget, LoadChains, LoadedModule, LocalImport,
    LocalImports, Outcome, Pair, Tree, Unparsed,
};
use facts::{Facts, Import, Runs};
use graph::{FileFacts, Scopes, SymbolGraph, enclosing_symbol, import_chain, is_package_entry};
use kernel::{Deadline, Span, checkpoint};
use syntax::{ImportRule, Language};

use crate::scan::line_count;
use crate::tree_scan::TreeScan;
use crate::{Error, in_metric_scope};

// Returns true when cycle_dodge reads the imports of `path` as the change's own: a tier-1 source
// file outside test and fixture paths, whose walker tells when an import runs.
pub(crate) fn in_dodge_scope(path: &str) -> bool {
    in_metric_scope(path) && Language::from_path(path).is_some_and(tells_runs)
}

// Only the Python walker sets Import.runs.
fn tells_runs(language: Language) -> bool {
    language.extraction().imports == ImportRule::Python
}

// The tree before the change is the scanned tree with each kept pair's imports from its old
// text: a file the change adds is absent, and a file it deletes is present.
pub(crate) fn cycle_dodge(scan: &TreeScan, pairs: &[&Pair]) -> Result<Outcome, Error> {
    let Some(repo) = &scan.facts else {
        let stopped = DodgeTree::Stopped {
            seconds: scan.seconds,
        };
        return Ok(checks::cycle_dodge(pairs, stopped)?);
    };
    let after: Vec<FileFacts> = repo.files.iter().map(import_facts).collect();
    let before = before_files(&repo.files, pairs);
    let (after_listed, before_listed): (Vec<&FileFacts>, Vec<&FileFacts>) =
        (after.iter().collect(), before.iter().collect());
    let (after_scopes, before_scopes) =
        (Scopes::build(&after_listed), Scopes::build(&before_listed));
    let after_loads = Loads::of(&after, &after_scopes);
    let before_loads = Loads::of(&before, &before_scopes);
    let imports = LocalImports {
        after: local_imports(&after, &after_scopes),
        before: local_imports(&before, &before_scopes),
        before_loads: loaded_modules(&before),
        loads_changed: after_loads.pairs() != before_loads.pairs(),
        unparsed: unparsed(pairs, &after, &before),
    };
    let mut chains = Chains {
        after: after_loads,
        before: before_loads,
        deadline: &scan.deadline,
        seconds: scan.seconds,
        found: HashMap::new(),
    };
    let tree = DodgeTree::Built {
        imports,
        chains: &mut chains,
    };
    Ok(checks::cycle_dodge(pairs, tree)?)
}

// What the check reads of a file: its imports and error spans, the manifest and modules that
// Rust paths resolve through, and, in scope, the symbols that name a statement's function.
fn import_facts(file: &FileFacts) -> FileFacts {
    let facts = &file.facts;
    let symbols = if in_dodge_scope(&file.path) || is_package_entry(&file.path) {
        facts.symbols.clone()
    } else {
        Vec::new()
    };
    FileFacts {
        path: file.path.clone(),
        language: file.language,
        line_count: file.line_count,
        facts: Facts {
            symbols,
            imports: facts.imports.clone(),
            errors: facts.errors.clone(),
            manifest: facts.manifest.clone(),
            modules: facts.modules.clone(),
            ..Facts::default()
        },
    }
}

// An old text that the parser refuses gives no file: the check then refuses that side.
fn before_files(files: &[FileFacts], pairs: &[&Pair]) -> Vec<FileFacts> {
    let changed: BTreeSet<&str> = pairs.iter().map(|pair| pair.path()).collect();
    let mut before: Vec<FileFacts> = files
        .iter()
        .filter(|file| !changed.contains(file.path.as_str()))
        .map(import_facts)
        .collect();
    before.extend(pairs.iter().filter_map(|pair| old_facts(pair)));
    before.sort_by(|a, b| a.path.cmp(&b.path));
    before
}

fn old_facts(pair: &Pair) -> Option<FileFacts> {
    let (language, text) = (pair.language()?, pair.old_text()?);
    let parsed = syntax::parse(language, Arc::from(text)).ok()?;
    let file = FileFacts {
        path: pair.path().to_owned(),
        language,
        line_count: line_count(text),
        facts: facts::extract(&parsed),
    };
    Some(import_facts(&file))
}

// Each side of a pair whose facts hold an error span, with the line where the first one starts.
fn unparsed(pairs: &[&Pair], after: &[FileFacts], before: &[FileFacts]) -> Vec<Unparsed> {
    let mut found = Vec::new();
    for (tree, files) in [(Tree::Before, before), (Tree::After, after)] {
        let errors: HashMap<&str, Span> = (files.iter())
            .filter_map(|file| Some((file.path.as_str(), *file.facts.errors.first()?)))
            .collect();
        for pair in pairs {
            let text = match tree {
                Tree::Before => pair.old_text(),
                Tree::After => pair.new_text(),
            };
            if let (Some(error), Some(text)) = (errors.get(pair.path()), text) {
                found.push(Unparsed {
                    path: pair.path().to_owned(),
                    tree,
                    line: line_at(text, error.start()),
                });
            }
        }
    }
    found
}

// The 1-based line of the byte at `offset`.
fn line_at(text: &str, offset: u32) -> u32 {
    let before = text.as_bytes().iter().take(offset as usize);
    let breaks = before.filter(|byte| **byte == b'\n').count();
    u32::try_from(breaks + 1).unwrap_or(u32::MAX)
}

// Each import that runs in a function body, in a file in scope, with the files it imports.
fn local_imports(files: &[FileFacts], scopes: &Scopes<'_>) -> Vec<LocalImport> {
    let mut found = Vec::new();
    for file in files.iter().filter(|file| in_dodge_scope(&file.path)) {
        let imports = &file.facts.imports;
        let Some(scope) = scopes.get(&file.path) else {
            continue;
        };
        let mut rows: BTreeMap<usize, Vec<(&str, &str)>> = BTreeMap::new();
        for (statement, module, target) in scope.resolved_statements(&file.path) {
            rows.entry(statement).or_default().push((module, target));
        }
        for (position, import) in imports.iter().enumerate() {
            if import.runs == Runs::Call {
                let targets = targets(import, rows.remove(&position).unwrap_or_default());
                found.push(local_import(file, import, targets));
            }
        }
    }
    found
}

// `from pkg import mod` imports the submodule; the package itself counts only when the statement
// names a member that no submodule holds, or names no member.
fn targets(import: &Import, rows: Vec<(&str, &str)>) -> Vec<ImportTarget> {
    let submodules = rows.iter().filter(|(module, _)| *module != import.module);
    let whole = import.names.is_empty() || submodules.count() < import.names.len();
    let dots = ".".repeat(import.relative_level as usize);
    rows.into_iter()
        .filter(|(module, _)| whole || *module != import.module)
        .map(|(module, target)| ImportTarget {
            module: format!("{dots}{module}"),
            path: target.to_owned(),
        })
        .collect()
}

fn local_import(file: &FileFacts, import: &Import, targets: Vec<ImportTarget>) -> LocalImport {
    let mut names = import.names.clone();
    names.sort_unstable();
    LocalImport {
        path: file.path.clone(),
        line: import.line,
        function: enclosing_symbol(&file.facts.symbols, import.line).map(facts::Symbol::qualname),
        module: import.module.clone(),
        level: import.relative_level,
        names,
        targets,
    }
}

fn loaded_modules(files: &[FileFacts]) -> BTreeSet<LoadedModule> {
    let in_scope = files.iter().filter(|file| in_dodge_scope(&file.path));
    in_scope
        .flat_map(|file| {
            let loaded = file.facts.imports.iter();
            loaded
                .filter(|import| import.runs == Runs::Load)
                .map(|import| LoadedModule {
                    path: file.path.clone(),
                    module: import.module.clone(),
                    level: import.relative_level,
                })
        })
        .collect()
}

// One tree's import graph, and the statement lines of its imports that do not run at load.
struct Loads<'a> {
    graph: SymbolGraph<'a>,
    late: BTreeSet<(&'a str, u32)>,
}

impl<'a> Loads<'a> {
    fn of(files: &'a [FileFacts], scopes: &Scopes<'a>) -> Loads<'a> {
        let late = files
            .iter()
            .flat_map(|file| {
                let late = file.facts.imports.iter();
                late.filter(|import| import.runs != Runs::Load)
                    .map(|import| (file.path.as_str(), import.line))
            })
            .collect();
        Loads {
            graph: SymbolGraph::imports(scopes),
            late,
        }
    }

    fn at_load(&self, path: &'a str, line: u32) -> bool {
        !self.late.contains(&(path, line))
    }

    // The pairs of files that the imports which run at load connect.
    fn pairs(&self) -> BTreeSet<(&'a str, &'a str)> {
        let edges = self.graph.import_edges();
        edges
            .filter(|edge| self.at_load(edge.source().path(), edge.source().line()))
            .map(|edge| (edge.source().path(), edge.target().path()))
            .collect()
    }

    fn search(&self, from: &str, to: &str) -> Option<Vec<ChainHop>> {
        let chain = import_chain(&self.graph, from, to, |path, line| self.at_load(path, line))?;
        let hops = chain.into_iter().map(|edge| ChainHop {
            path: edge.source().path().to_owned(),
            line: edge.source().line(),
            target: edge.target().path().to_owned(),
        });
        Some(hops.collect())
    }
}

// The chains of the two trees, each search found once per run. Each search checks the scan's
// deadline first.
struct Chains<'d, 'a> {
    after: Loads<'a>,
    before: Loads<'a>,
    deadline: &'d Deadline,
    seconds: u64,
    found: HashMap<(Tree, String, String), Option<Vec<ChainHop>>>,
}

impl LoadChains for Chains<'_, '_> {
    fn chain(
        &mut self,
        tree: Tree,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<ChainHop>>, BudgetSpent> {
        let key = (tree, from.to_owned(), to.to_owned());
        if let Some(found) = self.found.get(&key) {
            return Ok(found.clone());
        }
        let seconds = self.seconds;
        checkpoint(Some(self.deadline)).map_err(|_| BudgetSpent { seconds })?;
        let loads = match tree {
            Tree::After => &self.after,
            Tree::Before => &self.before,
        };
        let found = loads.search(from, to);
        self.found.insert(key, found.clone());
        Ok(found)
    }
}

#[cfg(test)]
mod tests;
