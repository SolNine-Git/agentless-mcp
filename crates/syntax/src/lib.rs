//! The vendored tree-sitter grammars, a parse with a flat node table, the language tables, and queries.

mod extraction;
mod ffi;
mod flat;
mod kinds;
mod language;
mod parsed;
mod query;
#[cfg(test)]
mod query_tests;
mod rules;
mod spec;
#[cfg(test)]
mod table_tests;
mod tables;
#[cfg(test)]
mod tests;

mod generated {
    pub mod digest;
    pub mod languages;
}

pub use extraction::{Declarations, Extraction, ImportBinding, ImportRule, RoleRule, SymbolRule};
pub use generated::digest::TABLES_DIGEST;
pub use generated::languages::EXTENSIONS;
pub use language::{FieldId, KindId, KindSet, Language};
pub use parsed::{NodeId, Parsed, Point, Unparsable, parse};
pub use query::{Capture, Match, Query, QueryError, QueryRunError};
pub use rules::rust_impl_owner;
pub use spec::{CloneRules, DecisionRules, Spec, UnitRules};
pub use tables::{LiteralClass, SlocRule, Tier};

/// The tree-sitter-language-pack release that the vendored grammar sources come from.
pub const PACK_VERSION: &str = "1.14.3";
