//! Caller-facing message templates and tool descriptions as typed data, and the contract rules.

mod rule;
mod template;
#[cfg(test)]
mod tests;

mod generated {
    #[cfg(test)]
    pub mod catalog;
    pub mod envelope;
    pub mod messages;
    pub mod parameter_descriptions;
    pub mod rules;
    pub mod tool_descriptions;
}

pub use generated::{envelope, messages, parameter_descriptions, tool_descriptions};
pub use rule::{ContractRule, Severity};
pub use template::{Count, Filled, Kind, Signature, Template, Text};

/// Returns every contract rule, one record per rule and language, in file order.
#[must_use]
pub fn contract_rules() -> &'static [ContractRule] {
    &generated::rules::RULES
}
