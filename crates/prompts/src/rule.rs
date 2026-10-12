//! The contract rule record.

/// How a contract rule's finding counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The finding is a warning.
    Warning,
    /// The finding is an advisory.
    Advisory,
}

/// One contract rule for one language: its id, severity, tree-sitter query and message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContractRule {
    /// The rule id, shared by the records of one rule.
    pub id: &'static str,
    /// The language whose sources the query matches.
    pub language: &'static str,
    /// How a finding of this rule counts.
    pub severity: Severity,
    /// The tree-sitter query that finds the code the rule flags.
    pub query: &'static str,
    /// Why the flagged code is a defect.
    pub message: &'static str,
}
