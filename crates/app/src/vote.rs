//! The vote: a verdicts document read back and ranked by equivalence cluster.

use patch::VoteReport;

use crate::Error;
use crate::verdicts::{LoadedVerdicts, load_verdicts};

/// A verdicts document and the ranking of its candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vote {
    /// The document as read.
    pub verdicts: LoadedVerdicts,
    /// The tier that answered and its clusters in rank order.
    pub report: VoteReport,
}

/// Reads `text` as a verdicts document and ranks its candidates, with the reproduction rung
/// only when the document says the reproduction command reproduced the bug.
///
/// # Errors
///
/// Returns the reader's operation failure, which names the line.
pub fn vote(text: &str) -> Result<Vote, Error> {
    let verdicts = load_verdicts(text)?;
    let report = patch::rank(&verdicts.candidates, verdicts.repro_valid);
    Ok(Vote { verdicts, report })
}

#[cfg(test)]
mod tests;
