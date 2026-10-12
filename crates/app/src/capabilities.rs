//! The capabilities report: the build, the grammars, the tag cache, the config, and the caps.

use std::fmt;
use std::sync::Arc;

use prompts::messages::CACHE_BUILD_HINT;
use serde::{Serialize, Serializer};
use store::CacheState;
use syntax::Language;

use crate::{CHECKS, Error, Session};

/// The grammar cache field: the grammars are part of the binary, so no directory holds them.
pub const GRAMMAR_CACHE: &str = "(linked into the binary)";
/// The token ceiling of one answer.
pub const MAX_OUTPUT_TOKENS: u64 = 16_000;
/// The most config warnings that one answer lists.
pub const MAX_CONFIG_WARNINGS: u64 = 8;

/// One bound that the services apply.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CapValue {
    /// A count.
    Int(u64),
    /// A real number.
    Float(f64),
}

impl fmt::Display for CapValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => f.write_str(&json::float_text(*value)),
        }
    }
}

const fn int(value: usize) -> CapValue {
    CapValue::Int(value as u64)
}

/// Every bound the services apply, in report order.
pub const CAPS: [(&str, CapValue); 27] = [
    ("max_walk_depth", int(io::MAX_WALK_DEPTH)),
    ("max_walk_files", int(io::MAX_WALK_FILES)),
    ("max_file_bytes", int(io::MAX_FILE_BYTES)),
    ("max_output_tokens", CapValue::Int(MAX_OUTPUT_TOKENS)),
    ("max_config_warnings", CapValue::Int(MAX_CONFIG_WARNINGS)),
    ("max_map_files", CapValue::Int(DEFAULT_MAX_FILES as u64)),
    ("max_test_companions", CapValue::Int(5)),
    ("test_companion_depth", CapValue::Int(2)),
    ("default_find_limit", CapValue::Int(20)),
    ("default_refs_limit", CapValue::Int(50)),
    ("max_expand_symbols", CapValue::Int(10)),
    ("default_explain_limit", CapValue::Int(20)),
    ("default_cycle_limit", CapValue::Int(20)),
    ("default_community_limit", CapValue::Int(20)),
    ("default_health_limit", CapValue::Int(20)),
    ("default_member_limit", CapValue::Int(12)),
    ("default_community_resolution", CapValue::Float(1.0)),
    ("default_context_lines", CapValue::Int(10)),
    ("default_tree_depth", CapValue::Int(4)),
    ("default_tree_entries", CapValue::Int(500)),
    ("default_diagram_nodes", CapValue::Int(40)),
    ("default_diagram_edges", CapValue::Int(40)),
    ("default_html_nodes", CapValue::Int(200)),
    ("default_html_edges", CapValue::Int(600)),
    ("max_html_nodes", CapValue::Int(1000)),
    ("max_html_edges", CapValue::Int(5000)),
    ("max_path_visited", CapValue::Int(20_000)),
];

// The map default that effective_config and the max_map_files cap report.
const DEFAULT_MAX_FILES: u32 = 10;
const TEST_CMD_HIDDEN: &str = "configured (CLI validate only; value hidden)";

/// One resolved project setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Setting {
    /// A text value.
    Text(String),
    /// A count.
    Int(u32),
    /// A switch.
    Bool(bool),
    /// A list of texts.
    List(Vec<String>),
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Int(value) => write!(f, "{value}"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::List(items) if items.is_empty() => f.write_str("none"),
            Self::List(items) => f.write_str(&items.join(", ")),
        }
    }
}

/// The capability of one linked grammar: its tier, its ABI, and the result of its probe parse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LanguageCapability {
    /// The language name.
    pub name: &'static str,
    /// The support tier, 1 or 2.
    pub tier: u8,
    /// The grammar's ABI version.
    pub abi: Option<usize>,
    /// True: every grammar is linked into the binary.
    pub warmed: bool,
    /// True when the probe sample parses into a tree with no error node.
    pub probe_ok: bool,
    /// Why the probe failed, or empty.
    pub detail: String,
}

/// The tag cache as the capabilities report describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CacheView {
    /// The `cache:` field of the receipt.
    pub receipt: String,
    /// The database path.
    pub path: String,
    /// The generation the database was built at.
    pub generation: Option<String>,
    /// The generation of the repository now.
    pub repo_generation: Option<String>,
    /// True when the two generations are equal.
    pub generation_matches: bool,
    /// False when the call bypassed the cache.
    pub enabled: bool,
    /// The indexed files.
    pub files: u64,
    /// The symbol rows.
    pub tags: u64,
    /// Why an existing database was not used, or empty.
    pub note: String,
}

/// The lint check and the input it reads, as the report lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LintCheckRow {
    /// The check's name.
    pub name: &'static str,
    /// The input it reads.
    pub input: &'static str,
}

/// The project config values and the text of each warning, in the config's JSON form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConfigView<'a> {
    /// The checked values.
    #[serde(flatten)]
    pub config: &'a io::ProjectConfig,
    /// Each warning's text, in load order.
    pub warnings: Vec<String>,
}

/// The allowlist and the client roots. The command line has neither.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Roots {
    /// The operator's allowlist, or `None` for none.
    pub configured: Option<Vec<String>>,
    /// The roots the client advertised.
    pub client: Vec<String>,
}

/// Everything the capabilities command reports, in its JSON key order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Capabilities<'a> {
    /// The build version.
    pub version: &'static str,
    /// The version of the grammar sources.
    pub pack_version: &'static str,
    /// Where the grammars come from.
    pub grammar_cache: &'static str,
    /// The tag cache.
    pub cache: CacheView,
    /// Each grammar, in name order.
    pub languages: Vec<LanguageCapability>,
    /// Each file suffix and its language, in suffix order.
    #[serde(serialize_with = "suffix_map")]
    pub extensions: &'static [(&'static str, Language)],
    /// The languages whose locals refs tells from callers.
    pub scope_analysed_languages: [&'static str; 1],
    /// Each lint check and its input, in registry order.
    pub lint_checks: Vec<LintCheckRow>,
    /// The project config.
    pub config: ConfigView<'a>,
    /// Each setting as the services resolve it.
    #[serde(serialize_with = "ordered_map")]
    pub effective_config: Vec<(&'static str, Setting)>,
    /// Each bound the services apply.
    #[serde(serialize_with = "ordered_map")]
    pub caps: &'static [(&'static str, CapValue)],
    /// The command that builds a missing cache, or empty.
    pub cache_hint: String,
    /// The roots.
    pub roots: Roots,
}

/// Builds the capabilities report of `session`.
///
/// # Errors
///
/// Returns the error of a row count that failed.
pub fn capabilities(session: &Session) -> Result<Capabilities<'_>, Error> {
    let config = &session.repo.config;
    Ok(Capabilities {
        version: env!("CARGO_PKG_VERSION"),
        pack_version: syntax::PACK_VERSION,
        grammar_cache: GRAMMAR_CACHE,
        cache: cache_view(session)?,
        languages: Language::ALL.into_iter().map(language).collect(),
        extensions: &syntax::EXTENSIONS,
        scope_analysed_languages: ["python"],
        lint_checks: CHECKS
            .iter()
            .map(|check| LintCheckRow {
                name: check.name,
                input: check.input.as_str(),
            })
            .collect(),
        config: ConfigView {
            config,
            warnings: config_warnings(config),
        },
        effective_config: effective_config(config),
        caps: &CAPS,
        cache_hint: cache_hint(session),
        roots: Roots {
            configured: None,
            client: Vec::new(),
        },
    })
}

/// Returns the text of each warning of `config`, in load order.
#[must_use]
pub fn config_warnings(config: &io::ProjectConfig) -> Vec<String> {
    config.warnings.iter().map(ToString::to_string).collect()
}

fn cache_view(session: &Session) -> Result<CacheView, Error> {
    let report = &session.cache;
    let counts = report.counts.clone().map_err(Error::Store)?;
    let status = &report.status;
    let (generation, repo_generation) = match &status.state {
        CacheState::Indexed {
            generation,
            repo_generation,
            ..
        } => (Some(generation.clone()), Some(repo_generation.clone())),
        _ => (None, None),
    };
    Ok(CacheView {
        receipt: status.receipt(),
        path: status.path.display().to_string(),
        generation,
        repo_generation,
        generation_matches: status.generation_matches(),
        enabled: status.state != CacheState::Bypassed,
        files: counts.files,
        tags: counts.tags,
        note: status.note(),
    })
}

// Quoted, because the hint hands the caller a command to run.
fn cache_hint(session: &Session) -> String {
    match &session.cache.status.state {
        CacheState::Absent { .. } => {
            let root = session.repo.root.display().to_string();
            CACHE_BUILD_HINT.render(&store::shell_quote(&root))
        }
        _ => String::new(),
    }
}

fn language(language: Language) -> LanguageCapability {
    let (probe_ok, detail) = probe(language);
    LanguageCapability {
        name: language.name(),
        tier: language.tier().number(),
        abi: Some(language.abi_version()),
        warmed: true,
        probe_ok,
        detail,
    }
}

fn probe(language: Language) -> (bool, String) {
    let parsed = match syntax::parse(language, Arc::from(language.probe_sample())) {
        Ok(parsed) => parsed,
        Err(error) => return (false, format!("probe parse raised: {error}")),
    };
    let root = parsed.root();
    if parsed.has_error(root) {
        return (false, "probe parse produced an ERROR node".to_owned());
    }
    if parsed.child_count(root) == 0 {
        return (false, "probe parse produced an empty tree".to_owned());
    }
    (true, String::new())
}

fn effective_config(config: &io::ProjectConfig) -> Vec<(&'static str, Setting)> {
    let source = config
        .path
        .as_ref()
        .map_or_else(|| "none".to_owned(), |path| path.display().to_string());
    let map_budget = config
        .map_budget
        .map_or_else(|| Setting::Text("auto".to_owned()), Setting::Int);
    let granularity = config
        .granularity
        .unwrap_or(io::Granularity::Function)
        .name();
    let test_cmd = if config
        .test_cmd
        .as_deref()
        .is_some_and(|cmd| !cmd.is_empty())
    {
        TEST_CMD_HIDDEN
    } else {
        "none"
    };
    let threshold = config
        .complexity_threshold
        .unwrap_or(metrics::DEFAULT_THRESHOLD);
    vec![
        ("source", Setting::Text(source)),
        ("map_budget", map_budget),
        (
            "max_files",
            Setting::Int(config.max_files.unwrap_or(DEFAULT_MAX_FILES)),
        ),
        ("granularity", Setting::Text(granularity.to_owned())),
        (
            "docstrings",
            Setting::Bool(config.docstrings.unwrap_or(false)),
        ),
        (
            "stoplist",
            Setting::List(config.stoplist.iter().cloned().collect()),
        ),
        ("test_cmd", Setting::Text(test_cmd.to_owned())),
        (
            "relation_weights",
            Setting::Bool(config.relation_weights.unwrap_or(false)),
        ),
        ("complexity_threshold", Setting::Int(threshold)),
        ("erosion_gate", gate_setting(config)),
        (
            "boundary_globs",
            Setting::List(config.boundary_globs.clone()),
        ),
    ]
}

// An object whose every entry was dropped leaves each check on the hook's default.
fn gate_setting(config: &io::ProjectConfig) -> Setting {
    match &config.erosion_gate {
        Some(modes) if !modes.is_empty() => Setting::List(
            modes
                .iter()
                .map(|(check, mode)| format!("{}={}", check.name(), mode.name()))
                .collect(),
        ),
        _ => Setting::Text("unset".to_owned()),
    }
}

fn suffix_map<S: Serializer>(
    extensions: &&'static [(&'static str, Language)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let entries = extensions.iter();
    serializer.collect_map(entries.map(|(suffix, language)| (*suffix, language.name())))
}

fn ordered_map<S, V, T>(entries: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
    T: AsRef<[(&'static str, V)]>,
{
    serializer.collect_map(entries.as_ref().iter().map(|(name, value)| (*name, value)))
}

#[cfg(test)]
mod tests;
