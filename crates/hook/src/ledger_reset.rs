//! ledger-reset: at a compaction, the removal of the main context's ledger entries, because the
//! compaction removes their cards from the context. A subagent's entries stay.

use std::io::Read;

use app::EnvConfig;
use kernel::SystemClock;

use crate::ledger;
use crate::run::{self, Facts, LEDGER_STORAGE_UNAVAILABLE, Outcome, Run};

const HOOK: &str = "ledger-reset";
const EVENT_NAME: &str = "SessionStart";

const RESET: &str = "reset";
const NOTHING_TO_RESET: &str = "nothing_to_reset";

/// Removes the main context's ledger entries for the `SessionStart` payload on `stdin`. It writes
/// nothing and returns 0.
pub fn ledger_reset(env: &EnvConfig, stdin: &mut dyn Read) -> u8 {
    let clock = SystemClock;
    answer(&Run::of(env, &clock), stdin);
    0
}

pub(crate) fn answer(run: &Run<'_>, stdin: &mut dyn Read) {
    run::answered(run, HOOK, EVENT_NAME, &mut std::io::sink(), |facts| {
        decide(run, stdin, facts)
    });
}

fn decide(run: &Run<'_>, stdin: &mut dyn Read, facts: &mut Facts) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    match ledger::reset(&run.ledger, &context.session) {
        Ok(true) => Ok(Outcome::Recorded(RESET)),
        Ok(false) => Err(NOTHING_TO_RESET),
        Err(()) => Err(LEDGER_STORAGE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests;
