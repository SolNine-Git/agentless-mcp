//! The capabilities report as text: the build, the cache, the languages, the checks and the caps.

use std::collections::BTreeMap;

use app::{Capabilities, LanguageCapability, LintCheckRow};

use crate::blocks::detail_suffix;
use crate::envelope::Listing;

impl Listing for Capabilities<'_> {
    fn items(&self) -> usize {
        self.languages.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.languages.truncate(kept);
        prefix
    }
}

pub(crate) fn render_capabilities(report: &Capabilities<'_>) -> String {
    let mut lines = head_lines(report);
    lines.push("languages (name:tier/abi):".to_owned());
    lines.extend(language_lines(&report.languages));
    lines.push("extensions (language: suffixes):".to_owned());
    lines.extend(extension_lines(report));
    lines.push(format!(
        "scope analysis (a parameter or local is told from a caller): {}",
        report.scope_analysed_languages.join(", ")
    ));
    lines.push("lint checks (CLI only), by the input each reads:".to_owned());
    lines.extend(lint_check_lines(&report.lint_checks));
    lines.push("effective project config:".to_owned());
    lines.extend(
        report
            .effective_config
            .iter()
            .map(|(name, setting)| format!("  {name} = {setting}")),
    );
    lines.push("caps:".to_owned());
    lines.extend(
        report
            .caps
            .iter()
            .map(|(name, value)| format!("  {name} = {value}")),
    );
    lines.join("\n") + "\n"
}

fn head_lines(report: &Capabilities<'_>) -> Vec<String> {
    let cache = &report.cache;
    let mut lines = vec![
        format!("agentless-mcp {}", report.version),
        format!(
            "pack {}  grammar cache {}",
            report.pack_version, report.grammar_cache
        ),
        format!("tag cache: {}", cache.receipt),
        format!(
            "  path {}  files {}  tags {}",
            cache.path, cache.files, cache.tags
        ),
    ];
    if !report.cache_hint.is_empty() {
        lines.push(report.cache_hint.clone());
    }
    let roots = &report.roots;
    let configured = roots.configured.as_deref().map_or_else(
        || "unrestricted (CLI mode)".to_owned(),
        |configured| joined_or(configured, "none configured"),
    );
    lines.push(format!("roots: {configured}"));
    lines.push(format!(
        "client roots: {}",
        joined_or(&roots.client, "none advertised")
    ));
    lines
}

fn joined_or(items: &[String], empty: &str) -> String {
    let joined = items.join(", ");
    if joined.is_empty() {
        empty.to_owned()
    } else {
        joined
    }
}

enum LanguageState {
    Ready,
    Unavailable,
    Exceptional,
}

// Ready and unavailable languages share one line each; any other state gets its own line.
fn language_lines(languages: &[LanguageCapability]) -> Vec<String> {
    let (mut ready, mut unavailable, mut exceptional) = (Vec::new(), Vec::new(), Vec::new());
    for language in languages {
        let entry = language_entry(language);
        match language_state(language) {
            LanguageState::Ready => ready.push(entry),
            LanguageState::Unavailable => unavailable.push(entry),
            LanguageState::Exceptional => exceptional.push(exception_line(language, &entry)),
        }
    }
    let mut lines = Vec::new();
    if !ready.is_empty() {
        lines.push(format!("  warmed+probe: {}", ready.join(", ")));
    }
    if !unavailable.is_empty() {
        lines.push(format!("  unavailable: {}", unavailable.join(", ")));
    }
    lines.extend(exceptional);
    lines
}

fn language_state(language: &LanguageCapability) -> LanguageState {
    if language.warmed && language.probe_ok && language.detail.is_empty() {
        LanguageState::Ready
    } else if !language.warmed && !language.probe_ok && language.detail.starts_with("not warmed:") {
        LanguageState::Unavailable
    } else {
        LanguageState::Exceptional
    }
}

// An ABI of 0 prints as "-", the same as an unknown ABI.
fn language_entry(language: &LanguageCapability) -> String {
    let abi = match language.abi {
        Some(abi) if abi != 0 => abi.to_string(),
        _ => "-".to_owned(),
    };
    format!("{}:{}/{abi}", language.name, language.tier)
}

fn exception_line(language: &LanguageCapability, entry: &str) -> String {
    let detail = detail_suffix(&language.detail);
    format!(
        "  warmed={} probe={}: {entry}{detail}",
        language.warmed, language.probe_ok
    )
}

// One line per language, in name order, with its suffixes sorted.
fn extension_lines(report: &Capabilities<'_>) -> Vec<String> {
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (suffix, language) in report.extensions {
        grouped.entry(language.name()).or_default().push(suffix);
    }
    grouped
        .into_iter()
        .map(|(language, mut suffixes)| {
            suffixes.sort_unstable();
            format!("  {language}: {}", suffixes.join(", "))
        })
        .collect()
}

// One line per input, in the order the registry first names it.
fn lint_check_lines(checks: &[LintCheckRow]) -> Vec<String> {
    let mut grouped: Vec<(&str, Vec<&str>)> = Vec::new();
    for check in checks {
        match grouped.iter_mut().find(|(input, _)| *input == check.input) {
            Some((_, names)) => names.push(check.name),
            None => grouped.push((check.input, vec![check.name])),
        }
    }
    grouped
        .iter()
        .map(|(input, names)| format!("  {input}: {}", names.join(", ")))
        .collect()
}
