//! The test-run port: one test command run to its end or its bound, and the SIGINT watch.

use std::ffi::OsStr;

use io::{TestCommand, TestRun, TestRunError};

use crate::EnvConfig;

/// Runs one test command, and reports whether SIGINT arrived during a run or a write.
pub trait TestRunner: Sync {
    /// Runs `command` to its end or its bound.
    ///
    /// # Errors
    ///
    /// Returns the [`TestRunError`] of a SIGINT, or of a failed capture file, wait or watch.
    fn run(&self, command: &TestCommand<'_>) -> Result<TestRun, TestRunError>;

    /// Returns true when SIGINT arrived while a test run or a repository write held it.
    fn interrupted(&self) -> bool;
}

/// The test runner of the start-up environment: [`io::run_test`] with the start-up variables,
/// and [`io::interrupted`].
#[derive(Clone, Debug)]
pub struct EnvRunner {
    env: EnvConfig,
}

impl EnvRunner {
    /// Returns the runner whose commands inherit their variables from `env`.
    #[must_use]
    pub fn new(env: &EnvConfig) -> EnvRunner {
        EnvRunner { env: env.clone() }
    }
}

impl TestRunner for EnvRunner {
    fn run(&self, command: &TestCommand<'_>) -> Result<TestRun, TestRunError> {
        io::run_test(command, |name| self.env.var(name).map(OsStr::to_os_string))
    }

    fn interrupted(&self) -> bool {
        io::interrupted()
    }
}
