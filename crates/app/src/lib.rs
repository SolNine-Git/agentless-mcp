//! The operations layer: the lint pair forms, capabilities, and the repository context they read.

mod capabilities;
mod env;
mod error;
mod lint;
mod noted;
mod pairs;
mod parallel;
mod proposed;
mod registry;
mod repo;
mod revision;
mod rows;
mod scope;
mod tags;

#[cfg(test)]
mod testing;

pub use capabilities::{
    CAPS, CacheView, CapValue, Capabilities, ConfigView, GRAMMAR_CACHE, LanguageCapability,
    LintCheckRow, MAX_CONFIG_WARNINGS, MAX_OUTPUT_TOKENS, Roots, Setting, capabilities,
    config_warnings,
};
pub use checks::{CheckGap, Detail, Finding, Observed, PairFinding, Severity};
pub use env::{EnvConfig, Services, UnreadCwd, WALK_BUDGET};
pub use error::Error;
pub use lint::{LintCandidate, LintForm, LintReport, LintRequest, lint};
pub use noted::Noted;
pub use proposed::After;
pub use registry::{
    CHECKS, CheckInput, FormKind, LintCheck, PairCheck, PairSelection, unknown_check_refusal,
};
pub use repo::{Repo, resolve};
pub use rows::COVERAGE;
pub use scope::{in_metric_scope, is_fixture_path, is_test_path};
pub use tags::{CacheReport, Opening, Session, Tags};
