use std::sync::OnceLock;

use crate::rules::{BindingRule, Cond, NameRule, OwnerRule, SkipRule};
use crate::tables::{
    CloneTable, DecisionTable, LiteralClass, Measured, Names, SlocRule, UnitTable,
};
use crate::{FieldId, KindId, KindSet, Language, NodeId, Parsed};

/// The measurement tables of one tier-1 language, with kind names resolved against its grammar.
pub struct Spec {
    pub units: UnitRules,
    pub decisions: DecisionRules,
    pub clones: CloneRules,
    pub sloc: SlocRule,
    pub comments: KindSet,
    pub statement_parents: Option<KindSet>,
}

/// The kinds that make units and scopes, and the rules that name them.
pub struct UnitRules {
    pub units: KindSet,
    pub scopes: KindSet,
    pub bound: KindSet,
    descend: Option<KindSet>,
    name: NameRule,
    binding: Option<BindingRule>,
    owner: Option<OwnerRule>,
}

/// The kinds that add to cyclomatic complexity, and the tests some of them must pass.
pub struct DecisionRules {
    pub points: KindSet,
    conditional: Vec<(KindSet, Cond)>,
    field_operators: Vec<(KindSet, Names)>,
    operator_field: Option<FieldId>,
    token_operators: Vec<(KindSet, KindSet)>,
}

/// The kinds that make clone candidates and the kinds that a clone key abstracts.
pub struct CloneRules {
    pub functions: KindSet,
    pub blocks: KindSet,
    pub identifiers: KindSet,
    pub indent_blocks: KindSet,
    skip: Option<SkipRule>,
    literals: Vec<(KindSet, LiteralClass)>,
}

static SPECS: [OnceLock<Option<Spec>>; 22] = [const { OnceLock::new() }; 22];

pub(crate) fn spec(language: Language) -> Option<&'static Spec> {
    SPECS[language as usize]
        .get_or_init(|| {
            language
                .table()
                .measured
                .map(|measured| resolve(language, measured))
        })
        .as_ref()
}

fn resolve(language: Language, measured: &Measured) -> Spec {
    Spec {
        units: UnitRules::resolve(language, &measured.units),
        decisions: DecisionRules::resolve(language, &measured.decisions),
        clones: CloneRules::resolve(language, &measured.clones),
        sloc: measured.sloc,
        comments: language.kinds(measured.comments),
        statement_parents: measured
            .statement_parents
            .map(|names| language.kinds(names)),
    }
}

impl UnitRules {
    fn resolve(language: Language, table: &UnitTable) -> UnitRules {
        UnitRules {
            units: language.kinds(table.units),
            scopes: language.kinds(table.scopes),
            bound: language.kinds(table.bound),
            descend: table.descend.map(|names| language.kinds(names)),
            name: table.name,
            binding: table.binding,
            owner: table.owner,
        }
    }

    /// Return whether the unit walk enters a node of `kind`: always, when the language has no list.
    #[must_use]
    pub fn descends_into(&self, kind: KindId) -> bool {
        self.descend
            .as_ref()
            .is_none_or(|descend| descend.contains(kind))
    }

    /// Return the node that names a unit or a scope.
    #[must_use]
    pub fn name_of(&self, parsed: &Parsed, node: NodeId) -> Option<NodeId> {
        self.name.name(parsed, node)
    }

    /// Return the name node of the binding that holds an anonymous callable.
    #[must_use]
    pub fn binding_of(&self, parsed: &Parsed, node: NodeId) -> Option<NodeId> {
        self.binding?.name(parsed, node)
    }

    /// Return the node that names the type that owns a method.
    #[must_use]
    pub fn owner_of(&self, parsed: &Parsed, node: NodeId) -> Option<NodeId> {
        self.owner?.owner(parsed, node)
    }
}

impl DecisionRules {
    fn resolve(language: Language, table: &DecisionTable) -> DecisionRules {
        let set = |name: &str| language.kinds(&[name]);
        DecisionRules {
            points: language.kinds(table.points),
            conditional: table
                .conditional
                .iter()
                .map(|(kind, cond)| (set(kind), *cond))
                .collect(),
            field_operators: table
                .field_operators
                .iter()
                .map(|(kind, spellings)| (set(kind), *spellings))
                .collect(),
            operator_field: language.field("operator"),
            token_operators: table
                .token_operators
                .iter()
                .map(|(kind, tokens)| (set(kind), language.kinds(tokens)))
                .collect(),
        }
    }

    /// Return whether the node passes its kind's test, or None when its kind has no test.
    #[must_use]
    pub fn condition(&self, parsed: &Parsed, node: NodeId) -> Option<bool> {
        let kind = parsed.kind(node);
        let (_, cond) = self
            .conditional
            .iter()
            .find(|(kinds, _)| kinds.contains(kind))?;
        Some(cond.holds(parsed, node))
    }

    /// Return the operator spellings that add a decision for a node of `kind`.
    #[must_use]
    pub fn operator_spellings(&self, kind: KindId) -> Option<&'static [&'static str]> {
        let (_, spellings) = self
            .field_operators
            .iter()
            .find(|(kinds, _)| kinds.contains(kind))?;
        Some(spellings)
    }

    /// Return the grammar's `operator` field.
    #[must_use]
    pub fn operator_field(&self) -> Option<FieldId> {
        self.operator_field
    }

    /// Return the anonymous token kinds that each add a decision under a node of `kind`.
    #[must_use]
    pub fn token_operators(&self, kind: KindId) -> Option<&KindSet> {
        let (_, tokens) = self
            .token_operators
            .iter()
            .find(|(kinds, _)| kinds.contains(kind))?;
        Some(tokens)
    }
}

impl CloneRules {
    fn resolve(language: Language, table: &CloneTable) -> CloneRules {
        CloneRules {
            functions: language.kinds(table.functions),
            blocks: language.kinds(table.blocks),
            identifiers: language.kinds(table.identifiers),
            indent_blocks: language.kinds(table.indent_blocks),
            skip: table.skip,
            literals: table
                .literals
                .iter()
                .map(|(class, kinds)| (language.kinds(kinds), *class))
                .collect(),
        }
    }

    /// Return whether the clone walk skips this candidate, while it still enters it.
    #[must_use]
    pub fn skips(&self, parsed: &Parsed, node: NodeId) -> bool {
        self.skip.is_some_and(|skip| skip.skips(parsed, node))
    }

    /// Return the class that a literal of `kind` stands for in a Type-2 key.
    #[must_use]
    pub fn literal_class(&self, kind: KindId) -> Option<LiteralClass> {
        self.literals
            .iter()
            .find(|(kinds, _)| kinds.contains(kind))
            .map(|(_, class)| *class)
    }
}
