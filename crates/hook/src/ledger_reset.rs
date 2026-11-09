//! ledger-reset: at a compaction, the removal of the main context's ledger entries, because the
//! compaction removes their cards from the context. A subagent's entries stay.

use std::io::Read;

use crate::ledger::{self, ResetFailed};
use crate::run::{self, Facts, LEDGER_STORAGE_UNAVAILABLE, Outcome, Run};

const RESET: &str = "reset";
const NOTHING_TO_RESET: &str = "nothing_to_reset";

pub(crate) fn decide(
    run: &Run<'_>,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    match ledger::reset(&run.ledger, &context.session) {
        Ok(true) => Ok(Outcome::Recorded(RESET)),
        Ok(false) => Err(NOTHING_TO_RESET),
        Err(ResetFailed::Untrusted) => Err(LEDGER_STORAGE_UNAVAILABLE),
        Err(ResetFailed::Removal(error)) => {
            facts.error = Some(io::os_error_str(&error));
            Err(LEDGER_STORAGE_UNAVAILABLE)
        }
    }
}

#[cfg(test)]
mod tests;
