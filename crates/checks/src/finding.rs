//! Findings, their machine-readable details, and the one order every check returns them in.

use kernel::Key;
use prompts::messages;
use serde::{Serialize, Serializer};
use syntax::Language;

use crate::schema::keyed;

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
    /// A `callee_overlap` finding's two functions and the callees they share, boxed because it
    /// is far larger than the other details.
    Overlap(Box<CalleeShare>),
    /// A `cycle_dodge` finding's import and the chain that imports its file back.
    Dodge(ImportDodge),
    /// A `reuse_candidates` finding's two functions and the action names they share, boxed
    /// because it is far larger than most details.
    Reuse(Box<NameShare>),
}

/// How `complexity_delta` paired a new unit with an old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

impl Match {
    /// Every pairing, in the order that the match pass counts list them.
    pub const ALL: [Match; 4] = [Self::Body, Self::Name, Self::Collision, Self::New];

    /// Return the pairing's JSON name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Name => "name",
            Self::Collision => "collision",
            Self::New => "new",
        }
    }
}

impl Serialize for Match {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// The numbers behind one unit's finding, with no old side for a new unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitDelta {
    /// The unit's qualified name in the new text.
    pub unit: String,
    /// How the unit was paired.
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneSite {
    /// The repo-relative path.
    pub path: String,
    /// The first line.
    pub line: u32,
    /// The last line.
    pub end_line: u32,
}

/// A `callee_overlap` finding: the changed function, the existing function it overlaps, and the
/// callees of each with the evidence tier of each edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalleeShare {
    /// The changed function's qualified name in the new text.
    pub unit: String,
    /// How the changed function was paired.
    pub via: Match,
    /// The existing function's qualified name.
    pub other: String,
    /// The existing function's repo-relative path.
    pub other_path: String,
    /// The existing function's first line.
    pub other_line: u32,
    /// How many other functions, besides the changed one, meet the rule for the existing one.
    pub family: usize,
    /// Each shared callee, in id order.
    pub shared: Vec<SharedCallee>,
    /// Each callee of the existing function, in id order.
    pub other_callees: Vec<TieredCallee>,
}

/// A callee that both functions call, with the tier of each function's edge to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedCallee {
    /// The callee's stable id.
    pub callee: String,
    /// The tier of the changed function's edge.
    pub tier: &'static str,
    /// The tier of the existing function's edge.
    pub other_tier: &'static str,
}

/// A callee of the existing function, with the tier of its edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TieredCallee {
    /// The callee's stable id.
    pub callee: String,
    /// The tier of the edge.
    pub tier: &'static str,
}

/// A `reuse_candidates` finding: the added function, the existing function whose action names
/// it shares, the score, and the shared names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameShare {
    /// The added function's qualified name.
    pub unit: String,
    /// The existing function's qualified name.
    pub other: String,
    /// The existing function's repo-relative path.
    pub other_path: String,
    /// The existing function's first line.
    pub other_line: u32,
    /// The IDF-weighted Jaccard score, with three decimals.
    pub score: String,
    /// Each shared name that counts toward the score, sorted.
    pub shared: Vec<String>,
}

/// A `cycle_dodge` finding: a function-local import, the file it imports, and the load-time
/// imports from that file back to the import's own file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportDodge {
    /// The module as the statement names it, with the dots of its level.
    pub module: String,
    /// The repository file that the statement imports.
    pub target: String,
    /// The qualified name of the declaration around the statement.
    pub function: String,
    /// What the change did to make the import close the cycle.
    pub via: DodgeVia,
    /// Each load-time import from the target back to the statement's file, in chain order.
    pub chain: Vec<ChainHop>,
}

/// What a change did to make a function-local import close a cycle at module level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DodgeVia {
    /// The import moved into the function from module level in the same file.
    Moved,
    /// The import is new in the function.
    New,
    /// The import did not change; the change made the chain back.
    ReturnPath,
}

impl DodgeVia {
    /// Return the value's JSON name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Moved => "moved",
            Self::New => "new",
            Self::ReturnPath => "return_path",
        }
    }
}

impl Serialize for DodgeVia {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One load-time import of a chain: the statement at `path:line`, which imports `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainHop {
    /// The repo-relative path of the importing file.
    pub path: String,
    /// The line of the statement.
    pub line: u32,
    /// The repo-relative path of the imported file.
    pub target: String,
}

keyed! {
    UnitDelta {
        UNIT: unit,
        MATCH: via,
        CC_OLD: cc_old,
        CC_NEW: cc_new,
        SLOC_OLD: sloc_old,
        SLOC_NEW: sloc_new,
        THRESHOLD: threshold,
    }
    DriftKey { RULE: rule, UNIT: unit, CAPTURE: capture }
    CloneCopy {
        KEY: key,
        LINE: line,
        END_LINE: end_line,
        SITES: sites,
        SITES_TOTAL: sites_total,
    }
    CloneSite { PATH: path, LINE: line, END_LINE: end_line }
    CalleeShare {
        UNIT: unit,
        MATCH: via,
        OTHER: other,
        OTHER_PATH: other_path,
        OTHER_LINE: other_line,
        FAMILY: family,
        SHARED: shared,
        OTHER_CALLEES: other_callees,
    }
    SharedCallee { CALLEE: callee, TIER: tier, OTHER_TIER: other_tier }
    TieredCallee { CALLEE: callee, TIER: tier }
    NameShare {
        UNIT: unit,
        OTHER: other,
        OTHER_PATH: other_path,
        OTHER_LINE: other_line,
        SCORE: score,
        SHARED: shared,
    }
    ImportDodge {
        MODULE: module,
        TARGET: target,
        FUNCTION: function,
        VIA: via,
        CHAIN: chain,
    }
    ChainHop { PATH: path, LINE: line, TARGET: target }
}

pub(crate) const UNMEASURABLE_SOURCE: &str = "UnmeasurableSource";

// The not_checked row of a tree check whose scan or later work reached its time budget.
pub(crate) fn budget_spent(
    check: &'static str,
    reads: &str,
    compared: &str,
    seconds: u64,
) -> PairFinding {
    let budget = usize::try_from(seconds).unwrap_or(usize::MAX);
    let message = messages::TREE_CHECK_STOPPED.render((check, reads, compared, budget));
    let evidence = messages::TREE_CHECK_STOPPED_EVIDENCE;
    not_checked(check, "", message, evidence, None)
}

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
