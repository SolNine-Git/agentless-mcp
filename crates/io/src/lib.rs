//! The tool's file and process operations, and the schema of the project config that it reads.
//! Each operation returns typed values.

mod append;
mod cache;
mod config;
mod contain;
mod files;
mod interrupt;
mod keys;
mod lock;
mod oserror;
mod random;
mod read;
mod replace;
mod runner;
mod spawn;
mod testrun;
mod txn;
mod walk;
mod warning;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(test)]
mod fixtures;

pub use append::{AppendError, append_line};
pub use cache::{CacheRoot, HomeUnknown, RelativeCacheHome, cache_root};
pub use config::{
    CONFIG_FILENAME, GateMode, GatedCheck, Granularity, MAX_BOUNDARY_GLOB_CHARS,
    MAX_BOUNDARY_GLOBS, MAX_BUDGET, MAX_MAX_FILES, MIN_BUDGET, MIN_MAX_FILES, ProjectConfig,
    load_config,
};
pub use contain::{
    Refusal, contained_file_size, contained_path, file_size, file_stays_inside, resolve,
};
pub use files::{
    create_owner_dir, create_private_dir, remove_file, remove_tree, touch, write_file,
};
pub use interrupt::{end_by_interrupt, interrupted};
pub use keys::parse_config;
pub use lock::{LockError, LockGuard, try_lock};
pub use oserror::{os_error_class_text, os_error_line, strerror};
pub use random::random_bytes;
pub use read::{MAX_FILE_BYTES, NotUtf8, Skipped, Utf8Reason, decode_strict, read_bounded};
pub use replace::replace_file;
pub use runner::{NotRun, Outcome, Spec, run};
pub use spawn::{Started, spawn_or_run};
pub use testrun::{
    CAPTURE_POLL, CAPTURE_SLACK, DEFAULT_MAX_CAPTURE, GroupNote, KILL_REAP, RunResult, RunStatus,
    TERM_GRACE, TEST_ENV_ALLOWLIST, TestCommand, TestRun, TestRunError, run_test,
};
pub use txn::{BACKUP_SUFFIX, Leftover, STAGING_SUFFIX, WriteError, WriteFailure, write_all};
pub use walk::{MAX_WALK_BYTES, MAX_WALK_DEPTH, MAX_WALK_FILES, Walk, WalkRefusal, WalkSkip, walk};
pub use warning::{
    ConfigWarning, FileRefusal, GateWarning, GlobsWarning, StoplistWarning, TestCmdWarning,
    UnknownKeyWarning, ValueWarning,
};

fn byte_count(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}
