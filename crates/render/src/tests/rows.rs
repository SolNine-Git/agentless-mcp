use std::path::{Path, PathBuf};
use std::str::FromStr;

use app::{
    CacheView, CapValue, Capabilities, CheckGap, ConfigView, Detail, Finding, LanguageCapability,
    LintCandidate, LintCheckRow, LintReport, Observed, PairFinding, Roots, Setting, Severity,
};
use checks::{
    CloneCopy, CloneSite, CloneState, DriftKey, HitState, Match, UnitChange, UnitDelta, UnitState,
};
use io::ProjectConfig;
use json::JsonValue;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::Receipt;

// The cases of one section of crates/render/fixtures/<file>.json.
pub(crate) fn cases<T: DeserializeOwned>(file: &str, section: &str) -> T {
    text::testing::cases(env!("CARGO_MANIFEST_DIR"), file, section)
}

// The report types hold static texts, so a test value outlives its fixture row.
fn leak(text: &str) -> &'static str {
    text.to_owned().leak()
}

// A config document and its path, as the fixture gives them.
type ConfigRow = (String, Option<String>);

fn parsed_config((document, path): &ConfigRow) -> ProjectConfig {
    let value = json::parse(document);
    let Ok(JsonValue::Object(pairs)) = &value else {
        panic!("{document}: not a JSON object");
    };
    io::parse_config(pairs, path.as_ref().map(PathBuf::from))
}

#[derive(Deserialize)]
pub(crate) struct ReceiptRow {
    root: String,
    head: Option<String>,
    tree: Option<String>,
    dirty: Option<usize>,
    cache: String,
    note: String,
    config: ConfigRow,
    warnings: Option<Vec<String>>,
}

impl ReceiptRow {
    pub(crate) fn config(&self) -> ProjectConfig {
        parsed_config(&self.config)
    }

    // Explicit warning texts replace the ones the config gives.
    pub(crate) fn receipt<'a>(&'a self, config: &'a ProjectConfig) -> Receipt<'a> {
        Receipt {
            root: Path::new(&self.root),
            head: self.head.as_deref(),
            tree: self.tree.as_deref(),
            dirty: self.dirty,
            cache: &self.cache,
            note: &self.note,
            config,
            warnings: self
                .warnings
                .clone()
                .unwrap_or_else(|| app::config_warnings(config)),
        }
    }
}

fn string(value: &Value) -> String {
    let text = value.as_str();
    text.unwrap_or_else(|| panic!("{value}: not text"))
        .to_owned()
}

fn text(row: &Value, key: &str) -> String {
    string(&row[key])
}

// A null value is `None`.
fn small(value: &Value) -> Option<u32> {
    let count = value.as_u64()?;
    Some(u32::try_from(count).unwrap_or_else(|error| panic!("{value}: {error}")))
}

fn optional(row: &Value, key: &str) -> Option<u32> {
    small(&row[key])
}

fn number(row: &Value, key: &str) -> u32 {
    optional(row, key).unwrap_or_else(|| panic!("{key}: no count in {row}"))
}

fn size(row: &Value, key: &str) -> usize {
    let count = usize::try_from(number(row, key));
    count.unwrap_or_else(|error| panic!("{key}: {error}"))
}

fn flag(row: &Value, key: &str) -> bool {
    let value = row[key].as_bool();
    value.unwrap_or_else(|| panic!("{key}: no boolean in {row}"))
}

fn list<T>(row: &Value, key: &str, item: fn(&Value) -> T) -> Vec<T> {
    present(row, key, item).unwrap_or_else(|| panic!("{key}: no list in {row}"))
}

// An absent key is `None`.
fn present<T>(row: &Value, key: &str, item: fn(&Value) -> T) -> Option<Vec<T>> {
    let items = row.get(key)?.as_array();
    Some(
        items
            .unwrap_or_else(|| panic!("{key}: not a list"))
            .iter()
            .map(item)
            .collect(),
    )
}

pub(crate) fn report(rows: &[Value]) -> LintReport {
    LintReport {
        candidates: rows.iter().map(candidate).collect(),
    }
}

fn candidate(row: &Value) -> LintCandidate {
    LintCandidate {
        id: text(row, "id"),
        findings: list(row, "findings", pair_finding),
        observed: row.get("observed").map(observed),
    }
}

fn pair_finding(row: &Value) -> PairFinding {
    let finding = Finding {
        check: leak(&text(row, "check")),
        severity: severity(&text(row, "severity")),
        message: text(row, "message"),
        path: text(row, "path"),
        line: number(row, "line"),
        evidence: text(row, "evidence"),
    };
    PairFinding {
        finding,
        detail: row.get("detail").map(detail),
    }
}

fn severity(name: &str) -> Severity {
    match name {
        "advisory" => Severity::Advisory,
        "warning" => Severity::Warning,
        "not_checked" => Severity::NotChecked,
        _ => panic!("{name}: no such severity"),
    }
}

fn pairing(row: &Value) -> Match {
    match text(row, "match").as_str() {
        "body" => Match::Body,
        "name" => Match::Name,
        "collision" => Match::Collision,
        "new" => Match::New,
        other => panic!("{other}: no such match"),
    }
}

// The fields a detail holds tell its kind.
fn detail(row: &Value) -> Detail {
    if row.get("threshold").is_some() {
        Detail::Unit(unit_delta(row))
    } else if row.get("rule").is_some() {
        Detail::Drift(DriftKey {
            rule: text(row, "rule"),
            unit: text(row, "unit"),
            capture: text(row, "capture"),
        })
    } else if row.get("reason").is_some() {
        Detail::Gap(gap(row))
    } else {
        Detail::Copy(clone_copy(row))
    }
}

fn unit_delta(row: &Value) -> UnitDelta {
    UnitDelta {
        unit: text(row, "unit"),
        via: pairing(row),
        cc_old: optional(row, "cc_old"),
        cc_new: number(row, "cc_new"),
        sloc_old: optional(row, "sloc_old"),
        sloc_new: number(row, "sloc_new"),
        threshold: number(row, "threshold"),
    }
}

fn gap(row: &Value) -> CheckGap {
    match text(row, "reason").as_str() {
        "no_boundary_globs" => CheckGap::NoBoundaryGlobs,
        "no_tag_cache" => CheckGap::NoTagCache,
        "no_rules_for_language" => {
            let name = text(row, "language");
            let language = syntax::Language::from_name(&name);
            CheckGap::NoRulesForLanguage(language.unwrap_or_else(|| panic!("{name}: no language")))
        }
        other => panic!("{other}: no such reason"),
    }
}

fn key<K: FromStr>(row: &Value) -> K {
    let hex = text(row, "key");
    hex.parse().unwrap_or_else(|_| panic!("{hex}: not a key"))
}

fn clone_copy(row: &Value) -> CloneCopy {
    CloneCopy {
        key: key(row),
        line: number(row, "line"),
        end_line: number(row, "end_line"),
        sites: list(row, "sites", clone_site),
        sites_total: size(row, "sites_total"),
    }
}

fn clone_site(row: &Value) -> CloneSite {
    CloneSite {
        path: text(row, "path"),
        line: number(row, "line"),
        end_line: number(row, "end_line"),
    }
}

fn observed(row: &Value) -> Observed {
    Observed {
        units_over_threshold: present(row, "units_over_threshold", unit_state),
        units_changed: present(row, "units_changed", unit_change),
        contract_keys: present(row, "contract_keys", hit_state),
        clone_instances: present(row, "clone_instances", clone_state),
    }
}

fn unit_state(row: &Value) -> UnitState {
    UnitState {
        path: text(row, "path"),
        unit: text(row, "unit"),
        line: number(row, "line"),
        cc: number(row, "cc"),
        sloc: number(row, "sloc"),
    }
}

fn unit_change(row: &Value) -> UnitChange {
    UnitChange {
        path: text(row, "path"),
        unit: text(row, "unit"),
        line: number(row, "line"),
        via: pairing(row),
        cc_old: optional(row, "cc_old"),
        cc_new: number(row, "cc_new"),
        sloc_old: optional(row, "sloc_old"),
        sloc_new: number(row, "sloc_new"),
    }
}

fn hit_state(row: &Value) -> HitState {
    HitState {
        path: text(row, "path"),
        rule: text(row, "rule"),
        unit: text(row, "unit"),
        capture: text(row, "capture"),
    }
}

fn clone_state(row: &Value) -> CloneState {
    CloneState {
        path: text(row, "path"),
        key: key(row),
        line: number(row, "line"),
        end_line: number(row, "end_line"),
    }
}

#[derive(Deserialize)]
pub(crate) struct CapabilityRow {
    version: String,
    pack_version: String,
    grammar_cache: String,
    cache: Value,
    cache_hint: String,
    languages: Vec<Value>,
    extensions: Vec<(String, String)>,
    scope_analysed_languages: [String; 1],
    lint_checks: Vec<(String, String)>,
    config: ConfigRow,
    effective_config: Vec<(String, Value)>,
    caps: Vec<(String, Value)>,
    roots: Value,
}

impl CapabilityRow {
    pub(crate) fn config(&self) -> ProjectConfig {
        parsed_config(&self.config)
    }

    pub(crate) fn capabilities<'a>(&self, config: &'a ProjectConfig) -> Capabilities<'a> {
        Capabilities {
            version: leak(&self.version),
            pack_version: leak(&self.pack_version),
            grammar_cache: leak(&self.grammar_cache),
            cache: cache_view(&self.cache),
            languages: self.languages.iter().map(language).collect(),
            extensions: self.extensions().leak(),
            scope_analysed_languages: [leak(&self.scope_analysed_languages[0])],
            lint_checks: self.lint_checks.iter().map(lint_check).collect(),
            config: ConfigView {
                config,
                warnings: app::config_warnings(config),
            },
            effective_config: self.settings(),
            caps: self.caps().leak(),
            cache_hint: self.cache_hint.clone(),
            roots: roots(&self.roots),
        }
    }

    fn extensions(&self) -> Vec<(&'static str, syntax::Language)> {
        let pair = |(suffix, name): &(String, String)| {
            let language = syntax::Language::from_name(name);
            (
                leak(suffix),
                language.unwrap_or_else(|| panic!("{name}: no language")),
            )
        };
        self.extensions.iter().map(pair).collect()
    }

    fn settings(&self) -> Vec<(&'static str, Setting)> {
        let pair = |(name, value): &(String, Value)| (leak(name), setting(value));
        self.effective_config.iter().map(pair).collect()
    }

    fn caps(&self) -> Vec<(&'static str, CapValue)> {
        let pair = |(name, value): &(String, Value)| (leak(name), cap_value(value));
        self.caps.iter().map(pair).collect()
    }
}

fn cache_view(row: &Value) -> CacheView {
    let generation = |key: &str| row[key].as_str().map(str::to_owned);
    CacheView {
        receipt: text(row, "receipt"),
        path: text(row, "path"),
        generation: generation("generation"),
        repo_generation: generation("repo_generation"),
        generation_matches: flag(row, "generation_matches"),
        enabled: flag(row, "enabled"),
        files: number(row, "files").into(),
        tags: number(row, "tags").into(),
        note: text(row, "note"),
    }
}

fn language(row: &Value) -> LanguageCapability {
    let tier = u8::try_from(number(row, "tier")).unwrap_or_else(|error| panic!("tier: {error}"));
    LanguageCapability {
        name: leak(&text(row, "name")),
        tier,
        abi: optional(row, "abi").is_some().then(|| size(row, "abi")),
        warmed: flag(row, "warmed"),
        probe_ok: flag(row, "probe_ok"),
        detail: text(row, "detail"),
    }
}

fn lint_check((name, input): &(String, String)) -> LintCheckRow {
    LintCheckRow {
        name: leak(name),
        input: leak(input),
    }
}

fn setting(value: &Value) -> Setting {
    match value {
        Value::String(text) => Setting::Text(text.clone()),
        Value::Bool(flag) => Setting::Bool(*flag),
        Value::Number(_) => {
            Setting::Int(small(value).unwrap_or_else(|| panic!("{value}: no count")))
        }
        Value::Array(items) => Setting::List(items.iter().map(string).collect()),
        Value::Null | Value::Object(_) => panic!("{value}: no setting"),
    }
}

// An integer is a count, and anything else is a real number.
fn cap_value(value: &Value) -> CapValue {
    match (value.as_u64(), value.as_f64()) {
        (Some(count), _) => CapValue::Int(count),
        (None, Some(real)) => CapValue::Float(real),
        (None, None) => panic!("{value}: no cap"),
    }
}

fn roots(row: &Value) -> Roots {
    let texts = |items: &Value| {
        items
            .as_array()
            .map(|items| items.iter().map(string).collect())
    };
    Roots {
        configured: texts(&row["configured"]),
        client: texts(&row["client"]).unwrap_or_else(|| panic!("client: no list in {row}")),
    }
}
