//! The hooks that write at most one reply on stdout and exit 0: the hooks of the trace surface,
//! the prompt map and the retired search gate. Erosion has its own entry, because its reply
//! carries an exit code and stderr.

use std::io::{Read, Write};

use app::hooks::{Hook, Retired};
use app::{EnvConfig, Services};
use kernel::SystemClock;

use crate::prompt_map::{self, prompt_map};
use crate::retired::retired;
use crate::run::{self, Facts, Outcome, Run};
use crate::{ledger_mark, ledger_reset, read_card, subagent};

// The decision of a trace-surface hook, with its job's in-process work bound in.
type Decide = fn(&Run<'_>, &Services, &mut dyn Read, &mut Facts) -> Result<Outcome, &'static str>;

/// A hook that writes at most one reply on stdout and exits 0.
#[derive(Clone, Copy, Debug)]
pub struct ContextHook(Named);

#[derive(Clone, Copy, Debug)]
enum Named {
    Trace(Hook, Decide),
    PromptMap,
    Retired,
}

impl ContextHook {
    /// Returns the hook that the subcommand `name` names, or None for erosion and an unknown name.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        if Retired::ALL.iter().any(|hook| hook.name() == name) {
            return Some(Self(Named::Retired));
        }
        let hook = Hook::ALL.into_iter().find(|hook| hook.name() == name)?;
        let decide: Decide = match hook {
            Hook::ReadCard => |run, services, stdin, facts| {
                read_card::decide(run, read_card::in_process, services, stdin, facts)
            },
            Hook::LedgerMark => |run, services, stdin, facts| {
                ledger_mark::decide(run, ledger_mark::in_process, services, stdin, facts)
            },
            Hook::BriefRecord => subagent::recorded,
            Hook::SubagentMap => |run, services, stdin, facts| {
                subagent::mapped(run, prompt_map::in_process, services, stdin, facts)
            },
            Hook::LedgerReset => |run, _, stdin, facts| ledger_reset::decide(run, stdin, facts),
            Hook::PromptMap => return Some(Self(Named::PromptMap)),
            Hook::Erosion => return None,
        };
        Some(Self(Named::Trace(hook, decide)))
    }

    /// Answers the payload on `stdin`, with the reply, if any, on `stdout`, and returns 0.
    pub fn run(
        self,
        env: &EnvConfig,
        services: &Services,
        stdin: &mut dyn Read,
        stdout: &mut dyn Write,
    ) -> u8 {
        let (hook, decide) = match self.0 {
            Named::Trace(hook, decide) => (hook, decide),
            Named::PromptMap => return prompt_map(env, services, stdin, stdout),
            Named::Retired => return retired(stdin),
        };
        let clock = SystemClock;
        let run = Run::of(hook, env, &clock);
        run::answered(&run, stdout, |facts| decide(&run, services, stdin, facts));
        0
    }
}
