//! The erosion pair checks: `complexity_delta`, `contract_drift`, `clone_delta`, and
//! `callee_overlap`.

mod callees;
mod clone;
mod complexity;
mod contract;
mod copies;
mod error;
mod finding;
mod globs;
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
pub use error::{Error, SideError};
pub use finding::{
    CalleeShare, CheckGap, CloneCopy, CloneSite, Detail, DriftKey, Finding, Match, PairFinding,
    Severity, SharedCallee, TieredCallee, UnitDelta,
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
