//! The scan of the working tree that the tree checks of one `lint --since` run share: one walk
//! and one parse of each file, made for the first check that reads it, under one time budget.

use std::path::Path;
use std::sync::Arc;

use kernel::Deadline;
use store::FactSource;

use crate::scan::RepoFacts;
use crate::{Error, Noted, Services};

// The scanned tree, and the deadline that each tree check's later work also runs on.
pub(crate) struct TreeScan {
    // The facts of every file, or `None` when the scan reached the budget.
    pub(crate) facts: Option<RepoFacts>,
    pub(crate) deadline: Deadline,
    // The time budget, in whole seconds, that a stopped row names.
    pub(crate) seconds: u64,
}

// The walk's warnings come with either outcome; a walk refusal is the error.
pub(crate) fn tree_scan(services: &Services, root: &Path) -> Noted<Result<TreeScan, Error>> {
    let budget = services.walk_budget;
    let deadline = Deadline::new(budget.as_secs_f64(), 0.0, Arc::clone(&services.clock));
    let source = FactSource::parsing();
    let scanned = RepoFacts::load_at(services, root, &source, Some(&deadline));
    scanned.map(|scanned| {
        let facts = match scanned {
            Ok(facts) => Some(facts),
            Err(Error::Kind(kernel::Error::CallStopped(_))) => None,
            Err(error) => return Err(error),
        };
        Ok(TreeScan {
            facts,
            deadline,
            seconds: budget.as_secs(),
        })
    })
}
