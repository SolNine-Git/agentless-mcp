//! The operations layer: the read and write operations, the lint pair forms, capabilities, the
//! repository context they read, the clock and token counter ports, the MCP tool surface table,
//! and the agent guide.

mod args;
pub mod bounds;
mod budget;
mod capabilities;
mod cards;
mod clock;
mod communities;
mod cycles;
mod detached;
mod diagram;
mod endpoints;
mod env;
mod error;
mod expand;
mod explain;
mod file_graph;
mod find;
mod guide;
mod health;
mod history;
mod html;
mod index;
mod lint;
mod map;
mod noted;
pub mod ops;
mod pairs;
mod parallel;
mod patches;
mod path;
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
pub mod surface;
mod symbol_graph;
mod tags;
mod tokens;
mod tree;
mod validate;
mod verdicts;
mod views;
mod vote;
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
pub use communities::{CommunitiesRequest, CommunityReport, CommunityRow, communities};
pub use cycles::{CycleReport, CycleRow, CyclesRequest, cycles};
pub use diagram::{
    ChartEdge, ChartGroup, ChartNode, DiagramRequest, DiagramView, Flowchart, diagram,
};
pub use env::{EnvConfig, Services, UnreadCwd, WALK_BUDGET};
pub use error::Error;
pub use expand::{ExpandRequest, ExpandResult, expand};
pub use explain::{EdgeRow, ExplainRequest, Explanation, ImportRow, TierGroup, explain};
pub use find::{FindRequest, FindResult, find};
pub use guide::{guide_text, section_names, section_text, unknown_section};
pub use health::{
    DiscountedTier, HealthComplexity, HealthComplexityRow, HealthReport, HealthRequest,
    HealthSymbol, health,
};
pub use history::{
    HistoryEntry, HistoryRequest, HistoryResult, ListEnd, history, history_entry_text,
};
pub use html::{HtmlEdge, HtmlGraph, HtmlNode, HtmlRequest, html};
pub use index::{EvictionReport, IndexFailure, IndexReport, IndexRequest, IndexSkip, index};
pub use lint::{
    CandidatesRequest, CheckSelection, LintCandidateInput, lint_candidates, load_lint_candidates,
    load_lint_diff,
};
pub use lint::{LintCandidate, LintForm, LintReport, LintRequest, lint};
pub use map::{
    BodyMapResult, Companion, Companions, MapEntry, MapFile, MapRequest, MapResult, body_map, map,
    map_rows_text,
};
pub use noted::Noted;
pub use patch::{
    ApplyResult, BlockError, DiffNote, Edit, EditOutcome, EditStatus, ParseResult, SyntaxVerdict,
    parse_blocks,
};
pub use patches::{
    ApplyMode, ApplyReport, CheckReport, FileCheck, NO_SUCH_FILE, NOT_UTF8, NormalizeReport,
    apply_patch, check_patch, load_edits, normalize_patch,
};
pub use path::{PathHop, PathPolicy, PathRequest, PathTrace, path};
pub use proposed::After;
pub use reading::Reading;
pub use refs::{
    CallerRef, RefGroup, RefListing, RefSite, RefsRequest, RefsResult, SharedCaller, refs,
};
pub use registry::{
    CHECKS, CheckInput, FormKind, LintCheck, PairCheck, PairSelection, unknown_check_refusal,
};
pub use repo::{Repo, expand_user, resolve, resolve_in_git};
pub use rows::COVERAGE;
pub use rowtext::{MODULE_LEVEL, ROW_INDENT, file_site, locator, omitted_line, stable_ids_line};
pub use scan::Skipped;
pub use scope::{in_metric_scope, is_fixture_path, is_test_path};
pub use tags::{CacheReport, Opening, Session, Tags};
pub use tokens::{Chars4, Cl100k, TokenCounter};
pub use tree::{TreeRequest, TreeView, subtree, tree};
pub use validate::{
    ApplyStatus, BaselineStatus, CandidateVerdict, DEFAULT_JOBS, DEFAULT_REPEAT_BASELINE,
    DEFAULT_TIMEOUT_SECONDS, EnvRunner, ReproVerdict, RunHeader, TestRunner, ValidateReport,
    ValidateRequest, Verdict, validate,
};
pub use verdicts::{LoadedVerdicts, load_verdicts};
pub use views::{
    FileView, LocateRequest, LocationView, LocsRequest, SkeletonRequest, SliceRequest,
    locate_symbol, resolve_locations, skeleton, slice,
};
pub use vote::{Vote, vote};
pub use warmup::{WarmupReport, WarmupRequest, WarmupRow, warmup};
