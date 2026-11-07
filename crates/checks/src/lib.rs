//! The erosion pair checks: `complexity_delta`, `contract_drift`, `clone_delta`,
//! `callee_overlap`, and `cycle_dodge`.

mod callees;
mod clone;
mod complexity;
mod contract;
mod copies;
mod dodges;
mod error;
mod finding;
mod index;
mod matching;
mod observed;
mod pair;
mod rules;
pub mod schema;
#[cfg(test)]
mod tests;

pub use callees::{Callee, CalleeTree, TreeFunction, callee_overlap};
pub use clone::clone_delta;
pub use complexity::complexity_delta;
pub use contract::contract_drift;
pub use dodges::{
    BudgetSpent, DodgeTree, ImportTarget, LoadChains, LoadedModule, LocalImport, LocalImports,
    Tree, Unparsed, cycle_dodge,
};
pub use error::{Error, SideError};
pub use finding::{
    CalleeShare, ChainHop, CheckGap, CloneCopy, CloneSite, Detail, DodgeVia, DriftKey, Finding,
    ImportDodge, Match, PairFinding, Severity, SharedCallee, TieredCallee, UnitDelta,
};
pub use index::{CloneIndex, IndexedFiles, clone_index};
pub use observed::{CloneState, HitState, Observed, Outcome, UnitChange, UnitState};
pub use pair::{Pair, Side, SideText, TextPair, measure_blocks, pair_language};

/// The name of the check that reports units a change grows past the complexity threshold.
pub const COMPLEXITY_DELTA: &str = "complexity_delta";
/// The name of the check that reports contract-rule hits a change adds to a boundary module.
pub const CONTRACT_DRIFT: &str = "contract_drift";
/// The name of the check that reports blocks a change copies.
pub const CLONE_DELTA: &str = "clone_delta";
/// The name of the check that reports functions a change adds that call what an existing one
/// calls.
pub const CALLEE_OVERLAP: &str = "callee_overlap";
/// The name of the check that reports function-local imports that would close an import cycle at
/// module level.
pub const CYCLE_DODGE: &str = "cycle_dodge";
