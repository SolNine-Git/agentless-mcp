//! Caller-facing message templates as typed data, and the contract rules.

mod rule;
mod template;
#[cfg(test)]
mod tests;

mod generated {
    #[cfg(test)]
    pub mod catalog;
    pub mod envelope;
    pub mod messages;
    pub mod rules;
}

pub use generated::{envelope, messages};
pub use rule::{ContractRule, Severity};
pub use template::{Count, Kind, Signature, Template, Text};

/// Returns every contract rule, one record per rule and language, in file order.
#[must_use]
pub fn contract_rules() -> &'static [ContractRule] {
    &generated::rules::RULES
}
