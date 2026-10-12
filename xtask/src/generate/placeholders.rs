//! The kind of value that each template placeholder takes, and the order of the values.

use std::collections::BTreeSet;

/// The value type of a placeholder: text or a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Count,
}

impl Kind {
    /// Returns the name of the prompts crate's marker type for this kind.
    pub fn marker(self) -> &'static str {
        match self {
            Kind::Text => "Text",
            Kind::Count => "Count",
        }
    }
}

// A name's position is its value order, so templates with the same names take them in one order.
pub const PLACEHOLDERS: [(&str, Kind); 75] = [
    ("tool", Kind::Text),
    ("operation", Kind::Text),
    ("operations", Kind::Text),
    ("stray", Kind::Text),
    ("missing", Kind::Text),
    ("accepted", Kind::Text),
    ("required", Kind::Text),
    ("surface", Kind::Text),
    ("root", Kind::Text),
    ("head", Kind::Text),
    ("dirty", Kind::Text),
    ("cache", Kind::Text),
    ("repo_root", Kind::Text),
    ("roots", Kind::Text),
    ("file", Kind::Text),
    ("config", Kind::Text),
    ("path", Kind::Text),
    ("paths", Kind::Text),
    ("revision", Kind::Text),
    ("command", Kind::Text),
    ("sha", Kind::Text),
    ("rule", Kind::Text),
    ("unit", Kind::Text),
    ("qualname", Kind::Text),
    ("target", Kind::Text),
    ("name", Kind::Text),
    ("pair_checks", Kind::Text),
    ("check", Kind::Text),
    ("checks", Kind::Text),
    ("kind", Kind::Text),
    ("query", Kind::Text),
    ("kinds", Kind::Text),
    ("language", Kind::Text),
    ("languages", Kind::Text),
    ("pattern", Kind::Text),
    ("seeds", Kind::Text),
    ("capture", Kind::Text),
    ("start", Kind::Count),
    ("end", Kind::Count),
    ("line", Kind::Count),
    ("shown", Kind::Count),
    ("shortened", Kind::Count),
    ("dropped", Kind::Count),
    ("requested", Kind::Count),
    ("seats", Kind::Count),
    ("count", Kind::Count),
    ("failed", Kind::Count),
    ("total", Kind::Count),
    ("found", Kind::Text),
    ("expected", Kind::Count),
    ("limit", Kind::Count),
    ("minimum", Kind::Count),
    ("maximum", Kind::Count),
    ("old_cc", Kind::Count),
    ("new_cc", Kind::Count),
    ("old_sloc", Kind::Count),
    ("new_sloc", Kind::Count),
    ("threshold", Kind::Count),
    ("budget", Kind::Count),
    ("bytes", Kind::Count),
    ("cap", Kind::Count),
    ("chars", Kind::Count),
    ("max_tokens", Kind::Count),
    ("status", Kind::Text),
    ("listed", Kind::Text),
    ("sites", Kind::Text),
    ("note", Kind::Text),
    ("reason", Kind::Text),
    ("detail", Kind::Text),
    ("error", Kind::Text),
    ("why", Kind::Text),
    ("warning", Kind::Text),
    ("summary", Kind::Text),
    ("modes", Kind::Text),
    ("severities", Kind::Text),
];

const TABLE: &str = "xtask/src/generate/placeholders.rs";

/// Returns the value position of a placeholder name, or `None` for a name the table lacks.
pub fn position(name: &str) -> Option<usize> {
    PLACEHOLDERS.iter().position(|(listed, _)| *listed == name)
}

/// Returns a problem for each table name that no template uses.
pub fn stale(used: &BTreeSet<&str>) -> Vec<String> {
    PLACEHOLDERS
        .iter()
        .filter(|(name, _)| !used.contains(name))
        .map(|(name, _)| {
            format!("{TABLE}: no template uses the placeholder {name:?}: remove it from the table")
        })
        .collect()
}

/// Returns the problem for a placeholder name that the table lacks.
pub fn unknown(name: &str) -> String {
    format!("the placeholder {{{name}}} is not in the type table in {TABLE}: add it with its kind")
}
