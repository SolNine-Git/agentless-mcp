use crate::extraction::{ImportBinding, ImportRule, RoleRule, SymbolRule};
use crate::rules::{BindingRule, Cond, NameRule, OwnerRule, SkipRule};

/// How much evidence stands behind a language's tables: tier one has the measurement tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    One,
    Two,
}

/// How a language's source lines count: by tree leaves, or by Python's tokenizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlocRule {
    Leaves,
    PythonTokens,
}

/// The class that a literal kind stands for in a Type-2 clone key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralClass {
    Str,
    Num,
    Int,
    Float,
    Bool,
    Null,
}

impl Tier {
    /// Return the tier as the number 1 or 2.
    #[must_use]
    pub fn number(self) -> u8 {
        match self {
            Tier::One => 1,
            Tier::Two => 2,
        }
    }
}

impl LiteralClass {
    /// Return the class's token text, such as `STR`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LiteralClass::Str => "STR",
            LiteralClass::Num => "NUM",
            LiteralClass::Int => "INT",
            LiteralClass::Float => "FLOAT",
            LiteralClass::Bool => "BOOL",
            LiteralClass::Null => "NONE",
        }
    }
}

pub(crate) type Names = &'static [&'static str];

// One language's row of the generated tables, with kinds by name.
pub(crate) struct Table {
    pub(crate) name: &'static str,
    pub(crate) tier: Tier,
    pub(crate) extensions: Names,
    pub(crate) probe: &'static str,
    pub(crate) comments: Names,
    pub(crate) measured: Option<&'static Measured>,
    pub(crate) extraction: &'static ExtractionTable,
}

pub(crate) struct ExtractionTable {
    pub(crate) prefix: &'static str,
    pub(crate) symbols: SymbolRule,
    pub(crate) imports: ImportRule,
    pub(crate) roles: RoleRule,
    pub(crate) identifiers: Names,
    pub(crate) bodies: Names,
    pub(crate) skeleton_functions: Names,
    pub(crate) builtins: Names,
    pub(crate) declarations: Option<&'static DeclarationTable>,
}

pub(crate) struct DeclarationTable {
    pub(crate) functions: Names,
    pub(crate) classes: Names,
    pub(crate) imports: Names,
    pub(crate) name_field: Option<&'static str>,
    pub(crate) import_path_field: Option<&'static str>,
    pub(crate) import_path_kind: &'static str,
    pub(crate) names: Names,
    pub(crate) class_bodies: Names,
    pub(crate) signature_from_header: bool,
    pub(crate) receiver_field: Option<&'static str>,
    pub(crate) import_binding: ImportBinding,
    pub(crate) modifiers: Names,
    pub(crate) bindings: Names,
    pub(crate) function_values: Names,
    pub(crate) constants: Names,
    pub(crate) constant_keywords: Names,
}

pub(crate) struct Measured {
    pub(crate) sloc: SlocRule,
    pub(crate) statement_parents: Option<Names>,
    pub(crate) units: UnitTable,
    pub(crate) decisions: DecisionTable,
    pub(crate) clones: CloneTable,
}

pub(crate) struct UnitTable {
    pub(crate) units: Names,
    pub(crate) scopes: Names,
    pub(crate) bound: Names,
    pub(crate) descend: Option<Names>,
    pub(crate) name: NameRule,
    pub(crate) binding: Option<BindingRule>,
    pub(crate) owner: Option<OwnerRule>,
}

pub(crate) struct DecisionTable {
    pub(crate) points: Names,
    pub(crate) conditional: &'static [(&'static str, Cond)],
    pub(crate) field_operators: &'static [(&'static str, Names)],
    pub(crate) token_operators: &'static [(&'static str, Names)],
}

pub(crate) struct CloneTable {
    pub(crate) functions: Names,
    pub(crate) blocks: Names,
    pub(crate) skip: Option<SkipRule>,
    pub(crate) identifiers: Names,
    pub(crate) indent_blocks: Names,
    pub(crate) literals: &'static [(LiteralClass, Names)],
}
