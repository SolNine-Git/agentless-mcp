//! The operations layer: the lint pair forms, capabilities, the repository context they read, the
//! clock and token counter ports, and the agent guide.

mod args;
pub mod bounds;
mod budget;
mod capabilities;
mod cards;
mod clock;
mod cycles;
mod env;
mod error;
mod expand;
mod find;
mod guide;
mod history;
mod index;
mod lint;
mod map;
mod noted;
pub mod ops;
mod pairs;
mod parallel;
mod proposed;
mod reading;
mod refs;
mod registry;
mod repo;
mod revision;
mod rows;
mod rowtext;
mod scan;
mod scope;
mod span;
mod tags;
mod tokens;
mod tree;
mod views;
mod warmup;

mod generated {
    pub(crate) mod guide;
}

#[cfg(test)]
mod testing;

pub use args::IntArg;
pub use capabilities::{
    CAPS, CacheView, CapValue, Capabilities, ConfigView, GRAMMAR_CACHE, LanguageCapability,
    LintCheckRow, MAX_CONFIG_WARNINGS, MAX_OUTPUT_TOKENS, Roots, Setting, capabilities,
    config_warnings,
};
pub use cards::{RationaleNode, SymbolCard, cards_text, rationale_nodes, symbol_card};
pub use checks::{CheckGap, Detail, Finding, Observed, PairFinding, Severity};
pub use clock::{Clock, SystemClock};
pub use cycles::{CycleReport, CycleRow, CyclesRequest, cycles};
pub use env::{EnvConfig, Services, UnreadCwd, WALK_BUDGET};
pub use error::Error;
pub use expand::{ExpandRequest, ExpandResult, expand};
pub use find::{FindRequest, FindResult, find};
pub use guide::{guide_text, section_names, section_text, unknown_section};
pub use history::{
    HistoryEntry, HistoryRequest, HistoryResult, ListEnd, history, history_entry_text,
};
pub use index::{EvictionReport, IndexFailure, IndexReport, IndexRequest, IndexSkip, index};
pub use lint::{LintCandidate, LintForm, LintReport, LintRequest, lint};
pub use map::{
    BodyMapResult, Companion, Companions, MapEntry, MapFile, MapRequest, MapResult, body_map, map,
    map_rows_text,
};
pub use noted::Noted;
pub use proposed::After;
pub use reading::Reading;
pub use refs::{
    CallerRef, RefGroup, RefListing, RefSite, RefsRequest, RefsResult, SharedCaller, refs,
};
pub use registry::{
    CHECKS, CheckInput, FormKind, LintCheck, PairCheck, PairSelection, unknown_check_refusal,
};
pub use repo::{Repo, resolve, resolve_in_git};
pub use rows::COVERAGE;
pub use rowtext::{MODULE_LEVEL, ROW_INDENT, file_site, locator, omitted_line, stable_ids_line};
pub use scan::Skipped;
pub use scope::{in_metric_scope, is_fixture_path, is_test_path};
pub use tags::{CacheReport, Opening, Session, Tags};
pub use tokens::{Chars4, Cl100k, TokenCounter};
pub use tree::{TreeRequest, TreeView, tree};
pub use views::{
    FileView, LocateRequest, LocationView, LocsRequest, SkeletonRequest, SliceRequest,
    locate_symbol, resolve_locations, skeleton, slice,
};
pub use warmup::{WarmupReport, WarmupRequest, WarmupRow, warmup};
