//! Caller-facing message templates and tool descriptions as typed data, and the contract and
//! comment rules.

mod comment_rule;
mod rule;
mod template;
#[cfg(test)]
mod tests;

mod generated {
    #[cfg(test)]
    pub mod catalog;
    pub mod comment_rules;
    pub mod envelope;
    pub mod messages;
    pub mod parameter_descriptions;
    pub mod rules;
    pub mod tool_descriptions;
}

pub use comment_rule::{
    CommentGap, CommentGroup, CommentRules, Declared, DocRules, Head, RunRules, Suppressed,
    SuppressionRules,
};
pub use generated::{envelope, messages, parameter_descriptions, tool_descriptions};
pub use rule::{ContractRule, Severity};
pub use template::{Count, Filled, Kind, Signature, Template, Text};

/// Returns every contract rule, one record per rule and language, in file order.
#[must_use]
pub fn contract_rules() -> &'static [ContractRule] {
    &generated::rules::RULES
}

/// Returns the comment rules of every language, one record per language, in name order.
#[must_use]
pub fn comment_rules() -> &'static [CommentRules] {
    &generated::comment_rules::COMMENT_RULES
}
