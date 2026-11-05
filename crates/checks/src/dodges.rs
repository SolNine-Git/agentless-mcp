//! `cycle_dodge`: each import in a function body that would close an import cycle at module level,
//! when the change made it so.

use std::collections::{BTreeMap, BTreeSet};

use prompts::messages;

use crate::contract::MODULE_UNIT;
use crate::error::Error;
use crate::finding::{
    ChainHop, Detail, DodgeVia, Finding, ImportDodge, PairFinding, Severity, budget_spent, sorted,
};
use crate::observed::{Observed, Outcome};
use crate::pair::{DODGE_GAPS, Pair, each_pair, read_sides};

/// What `cycle_dodge` reads of the trees before and after a change, or the stop of its scan.
pub enum DodgeTree<'c> {
    /// The scan read both trees.
    Built {
        /// The function-local imports of the two trees.
        imports: LocalImports,
        /// The load-time import chains of the two trees.
        chains: &'c mut dyn LoadChains,
    },
    /// The scan reached the time budget of `seconds`.
    Stopped {
        /// The time budget, in whole seconds.
        seconds: u64,
    },
}

/// The imports in function bodies of the two trees of a change, in the files the check reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalImports {
    /// Each one of the tree after the change.
    pub after: Vec<LocalImport>,
    /// Each one of the tree before the change.
    pub before: Vec<LocalImport>,
    /// Each module that a file imports when it loads, in the tree before the change.
    pub before_loads: BTreeSet<LoadedModule>,
    /// Whether the imports that run at load connect other pairs of files after the change.
    pub loads_changed: bool,
}

/// One import statement in a function body, and the repository files it imports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImport {
    /// The repo-relative path.
    pub path: String,
    /// The line of the statement.
    pub line: u32,
    /// The qualified name of the innermost declaration around the statement, if any.
    pub function: Option<String>,
    /// The module that the statement names, without the dots of its level.
    pub module: String,
    /// The relative level: the number of leading dots.
    pub level: u32,
    /// The names that the statement imports from the module, sorted.
    pub names: Vec<String>,
    /// Each repository file that it imports, other than its own, in statement order.
    pub targets: Vec<ImportTarget>,
}

/// One repository file that an import statement imports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportTarget {
    /// The module as the statement names it, with the dots of its level.
    pub module: String,
    /// The repo-relative path of the file.
    pub path: String,
}

/// A module that a file imports when it loads.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LoadedModule {
    /// The repo-relative path of the importing file.
    pub path: String,
    /// The module that the statement names, without the dots of its level.
    pub module: String,
    /// The relative level of the statement.
    pub level: u32,
}

/// The tree of a change that an import chain runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tree {
    /// The tree before the change.
    Before,
    /// The tree after the change.
    After,
}

/// The search for a load-time import chain reached the time budget of `seconds`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetSpent {
    /// The time budget, in whole seconds.
    pub seconds: u64,
}

/// The load-time import chains of the two trees of a change.
pub trait LoadChains {
    /// Return the shortest chain of imports that run at load from the file `from` to the file
    /// `to` in `tree`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns the budget when the search reached it.
    fn chain(
        &mut self,
        tree: Tree,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<ChainHop>>, BudgetSpent>;
}

/// Report each import in a function body of the tree after the change whose file the imported
/// file imports back at load, when the tree before the change held no such statement and chain.
///
/// # Errors
///
/// Returns an error when two pairs name one path, or when a side fails in a way that is not a
/// parse or tokenizer failure.
pub fn cycle_dodge(pairs: &[&Pair], tree: DodgeTree<'_>) -> Result<Outcome, Error> {
    let (imports, chains) = match tree {
        DodgeTree::Stopped { seconds } => return Ok(stopped(seconds)),
        DodgeTree::Built { imports, chains } => (imports, chains),
    };
    let (_, mut findings) = each_pair(pairs, |pair, language| {
        read_sides(pair, language, &DODGE_GAPS)
    })?;
    let unread: BTreeSet<String> = findings
        .iter()
        .map(|row| row.finding.path.clone())
        .collect();
    match dodges(&imports, chains, &unread) {
        Ok(rows) => findings.extend(rows),
        Err(spent) => return Ok(stopped(spent.seconds)),
    }
    Ok(Outcome {
        findings: sorted(findings),
        observed: Observed::default(),
    })
}

// A partial search can miss the chain of the tree before the change and make a false row, so a
// stop gives one not_checked row and no finding.
fn stopped(seconds: u64) -> Outcome {
    let row = budget_spent(crate::CYCLE_DODGE, messages::CYCLE_DODGE_READS, seconds);
    Outcome {
        findings: vec![row],
        observed: Observed::default(),
    }
}

// A statement pairs with the statements of the tree before the change that have its identity.
type Identity<'a> = (&'a str, Option<&'a str>, &'a str, u32, &'a [String]);

fn identity(statement: &LocalImport) -> Identity<'_> {
    (
        &statement.path,
        statement.function.as_deref(),
        &statement.module,
        statement.level,
        &statement.names,
    )
}

// The statements in a file with a side that does not parse are not compared: that file has a
// not_checked row.
fn dodges(
    imports: &LocalImports,
    chains: &mut dyn LoadChains,
    unread: &BTreeSet<String>,
) -> Result<Vec<PairFinding>, BudgetSpent> {
    let mut before: BTreeMap<Identity<'_>, Vec<&LocalImport>> = BTreeMap::new();
    for statement in &imports.before {
        before
            .entry(identity(statement))
            .or_default()
            .push(statement);
    }
    let mut rows = Vec::new();
    let compared = imports
        .after
        .iter()
        .filter(|statement| !unread.contains(&statement.path));
    for statement in compared {
        let old = before.get(&identity(statement)).map(Vec::as_slice);
        rows.extend(dodge(imports, chains, statement, old)?);
    }
    Ok(rows)
}

// The row of one statement of the tree after the change, when the change made it close a cycle.
// An old statement whose targets and load-time graph did not change cannot be one.
fn dodge(
    imports: &LocalImports,
    chains: &mut dyn LoadChains,
    statement: &LocalImport,
    old: Option<&[&LocalImport]>,
) -> Result<Option<PairFinding>, BudgetSpent> {
    let via = match old {
        None if loaded_before(imports, statement) => DodgeVia::Moved,
        None => DodgeVia::New,
        Some(old) if !imports.loads_changed && same_targets(old, statement) => return Ok(None),
        Some(_) => DodgeVia::ReturnPath,
    };
    let Some((target, chain)) = first_chain(chains, Tree::After, statement)? else {
        return Ok(None);
    };
    for held in old.unwrap_or_default() {
        if first_chain(chains, Tree::Before, held)?.is_some() {
            return Ok(None);
        }
    }
    Ok(Some(finding(statement, target, chain, via)))
}

// The file imported the same module at load before the change.
fn loaded_before(imports: &LocalImports, statement: &LocalImport) -> bool {
    let loaded = LoadedModule {
        path: statement.path.clone(),
        module: statement.module.clone(),
        level: statement.level,
    };
    imports.before_loads.contains(&loaded)
}

fn same_targets(old: &[&LocalImport], statement: &LocalImport) -> bool {
    old.iter().all(|held| held.targets == statement.targets)
}

// The first target, in statement order, that imports the statement's file back at load.
fn first_chain<'s>(
    chains: &mut dyn LoadChains,
    tree: Tree,
    statement: &'s LocalImport,
) -> Result<Option<(&'s ImportTarget, Vec<ChainHop>)>, BudgetSpent> {
    for target in &statement.targets {
        if let Some(chain) = chains.chain(tree, &target.path, &statement.path)? {
            return Ok(Some((target, chain)));
        }
    }
    Ok(None)
}

fn finding(
    statement: &LocalImport,
    target: &ImportTarget,
    chain: Vec<ChainHop>,
    via: DodgeVia,
) -> PairFinding {
    let function = statement.function.as_deref().unwrap_or(MODULE_UNIT);
    let path = statement.path.as_str();
    let mut cycle = vec![path];
    cycle.extend(chain.iter().map(|hop| hop.path.as_str()));
    cycle.push(path);
    let hops: Vec<String> = chain
        .iter()
        .map(|hop| format!("{}:{}", hop.path, hop.line))
        .collect();
    let message = messages::CYCLE_DODGE_FOUND.render((
        target.module.as_str(),
        function,
        cycle.join(" -> ").as_str(),
    ));
    let evidence = messages::CYCLE_DODGE_FOUND_EVIDENCE.render((
        path,
        target.path.as_str(),
        hops.join(" -> ").as_str(),
        statement.line as usize,
    ));
    let detail = ImportDodge {
        module: target.module.clone(),
        target: target.path.clone(),
        function: function.to_owned(),
        via,
        chain,
    };
    PairFinding {
        finding: Finding {
            check: crate::CYCLE_DODGE,
            severity: Severity::Advisory,
            message,
            path: path.to_owned(),
            line: statement.line,
            evidence,
        },
        detail: Some(Detail::Dodge(detail)),
    }
}

#[cfg(test)]
mod tests;
