//! Findings, their machine-readable details, and the one order every check returns them in.

use kernel::Key;
use serde::{Serialize, Serializer};
use syntax::Language;

/// How much attention a finding asks for; `NotChecked` says that a check did not run, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A finding to read.
    Advisory,
    /// A finding to act on.
    Warning,
    /// A statement that a check did not cover a file.
    NotChecked,
}

impl Severity {
    /// Every severity, in the order that help texts and refusals list them.
    pub const ALL: [Severity; 3] = [Self::Advisory, Self::Warning, Self::NotChecked];

    /// Return the severity's JSON name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Advisory => "advisory",
            Self::Warning => "warning",
            Self::NotChecked => "not_checked",
        }
    }
}

/// One thing a reviewer should look at: `path` is empty for the repository, `line` 0 for no line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The name of the check.
    pub check: &'static str,
    /// The finding's severity.
    pub severity: Severity,
    /// The caller-facing message.
    pub message: String,
    /// The repo-relative path, or empty for the repository.
    pub path: String,
    /// The 1-based line, or 0 when the finding has no line.
    pub line: u32,
    /// The short evidence text.
    pub evidence: String,
}

impl Finding {
    /// Return where the finding points: `(repository)`, the path, or `path:line`.
    #[must_use]
    pub fn location(&self) -> String {
        if self.path.is_empty() {
            return "(repository)".to_owned();
        }
        if self.line == 0 {
            return self.path.clone();
        }
        format!("{}:{}", self.path, self.line)
    }
}

#[derive(Serialize)]
struct FindingRow<'a> {
    check: &'a str,
    severity: Severity,
    message: &'a str,
    path: &'a str,
    line: u32,
    location: String,
    evidence: &'a str,
}

impl Serialize for Finding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let row = FindingRow {
            check: self.check,
            severity: self.severity,
            message: &self.message,
            path: &self.path,
            line: self.line,
            location: self.location(),
            evidence: &self.evidence,
        };
        row.serialize(serializer)
    }
}

/// A pair-check finding, and the detail behind it when it has one; its JSON is one flat row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairFinding {
    /// The finding.
    #[serde(flatten)]
    pub finding: Finding,
    /// The machine-readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Detail>,
}

/// The machine-readable detail of a pair-check finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Detail {
    /// A `complexity_delta` finding's numbers.
    Unit(UnitDelta),
    /// A `contract_drift` finding's hit identity.
    Drift(DriftKey),
    /// Why a check did not check a change, by design.
    Gap(CheckGap),
    /// A `clone_delta` finding's block and its other instances.
    Copy(CloneCopy),
}

/// How `complexity_delta` paired a new unit with an old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    /// The body is the same, with the unit's own name left out.
    Body,
    /// One old and one new unit have the name.
    Name,
    /// Several units have the name, paired by rank.
    Collision,
    /// No old unit pairs with it.
    New,
}

/// The numbers behind one unit's finding, with no old side for a new unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnitDelta {
    /// The unit's qualified name in the new text.
    pub unit: String,
    /// How the unit was paired.
    #[serde(rename = "match")]
    pub via: Match,
    /// The old complexity.
    pub cc_old: Option<u32>,
    /// The new complexity.
    pub cc_new: u32,
    /// The old source line count.
    pub sloc_old: Option<u32>,
    /// The new source line count.
    pub sloc_new: u32,
    /// The complexity threshold of the run.
    pub threshold: u32,
}

/// One contract hit's identity: the rule, the unit that holds it, and its normalized text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DriftKey {
    /// The rule id.
    pub rule: String,
    /// The owning unit's qualified name, or `<module>`.
    pub unit: String,
    /// The hit's text with comments cut out and whitespace collapsed.
    pub capture: String,
}

/// Why a pair check did not check a change, by design, as a reason a caller can match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckGap {
    /// The project config names no boundary globs.
    NoBoundaryGlobs,
    /// The language has no contract rules.
    NoRulesForLanguage(Language),
    /// The run has no tag cache to read clone blocks from.
    NoTagCache,
}

impl CheckGap {
    /// Return the reason's JSON name.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoBoundaryGlobs => "no_boundary_globs",
            Self::NoRulesForLanguage(_) => "no_rules_for_language",
            Self::NoTagCache => "no_tag_cache",
        }
    }
}

#[derive(Serialize)]
struct GapRow {
    reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'static str>,
}

impl Serialize for CheckGap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let language = match self {
            Self::NoRulesForLanguage(language) => Some(language.name()),
            Self::NoBoundaryGlobs | Self::NoTagCache => None,
        };
        let row = GapRow {
            reason: self.reason(),
            language,
        };
        row.serialize(serializer)
    }
}

/// A `clone_delta` finding's block: its span, its Type-2 key, and its other instances.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CloneCopy {
    /// The Type-2 key.
    pub key: Key,
    /// The first line of the block.
    pub line: u32,
    /// The last line of the block.
    pub end_line: u32,
    /// The first other instances, in path and line order.
    pub sites: Vec<CloneSite>,
    /// The number of other instances.
    pub sites_total: usize,
}

/// One other instance of a copied block, in the repository after the change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CloneSite {
    /// The repo-relative path.
    pub path: String,
    /// The first line.
    pub line: u32,
    /// The last line.
    pub end_line: u32,
}

pub(crate) const UNMEASURABLE_SOURCE: &str = "UnmeasurableSource";

pub(crate) fn not_checked(
    check: &'static str,
    path: &str,
    message: String,
    evidence: &str,
    gap: Option<CheckGap>,
) -> PairFinding {
    let finding = Finding {
        check,
        severity: Severity::NotChecked,
        message,
        path: path.to_owned(),
        line: 0,
        evidence: evidence.to_owned(),
    };
    PairFinding {
        finding,
        detail: gap.map(Detail::Gap),
    }
}

pub(crate) fn sorted(mut findings: Vec<PairFinding>) -> Vec<PairFinding> {
    findings.sort_by(|a, b| order(&a.finding).cmp(&order(&b.finding)));
    findings
}

fn order(finding: &Finding) -> (&str, u32, &str, &str) {
    (
        &finding.path,
        finding.line,
        &finding.message,
        &finding.evidence,
    )
}
