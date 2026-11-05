use crate::{FieldCursor, FieldId, Language, NodeId, Parsed};

const C_NAMES: [&str; 1] = ["identifier"];
const CPP_NAMES: [&str; 7] = [
    "identifier",
    "field_identifier",
    "qualified_identifier",
    "destructor_name",
    "operator_name",
    "operator_cast",
    "template_function",
];
const DECLARATOR_WRAPPERS: [&str; 2] = ["parenthesized_declarator", "attributed_declarator"];
const JAVASCRIPT_BINDINGS: [(&str, &str); 3] = [
    ("variable_declarator", "name"),
    ("field_definition", "property"),
    ("pair", "key"),
];
const TYPESCRIPT_BINDINGS: [(&str, &str); 3] = [
    ("variable_declarator", "name"),
    ("public_field_definition", "name"),
    ("pair", "key"),
];
const ECMASCRIPT_BINDING_NAMES: [&str; 4] = [
    "identifier",
    "property_identifier",
    "private_property_identifier",
    "string",
];
const LUA_BINDING_NAMES: [&str; 2] = ["identifier", "dot_index_expression"];
// A named type first, then a primitive one: `impl Trait for i32` owns `i32.f`.
const RUST_OWNER_KINDS: [&str; 2] = ["type_identifier", "primitive_type"];
// The conditions of a Python `if` block that only a type checker runs, in every file.
const TYPE_CHECKING: [&str; 2] = ["TYPE_CHECKING", "typing.TYPE_CHECKING"];
const TYPING: &str = "typing";
const TYPE_CHECKING_NAME: &str = "TYPE_CHECKING";

/// The spellings of an `if` condition that only a type checker runs, in one Python file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeChecking {
    aliases: Vec<String>,
}

/// The candidates of one parse that the clone walk skips, while it still enters them.
pub struct Skipped<'p> {
    parsed: &'p Parsed,
    type_checking: Option<TypeChecking>,
}

// The fields of a Python import statement that a TYPE_CHECKING alias is read from.
struct ImportFields {
    name: FieldId,
    alias: FieldId,
    module: FieldId,
}

// The rule that finds a unit's or a scope's name node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameRule {
    Field,
    CDeclarator,
    CppDeclarator,
    RustImpl,
}

// The rule that names an anonymous callable from the binding that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingRule {
    Javascript,
    Typescript,
    Lua,
}

// The rule that finds the type that owns a method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerRule {
    GoReceiver,
}

// The rule that skips a clone candidate while the walk still enters it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkipRule {
    TypeChecking,
}

// The structural test that decides whether a decision candidate counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cond {
    HasNamedChild,
    NotDefaultCase,
    CountedRustArm,
    HasCondition,
    CountedRubyArm,
    CountedCaseItem,
}

/// Return the node that names the type a Rust `impl` block implements: its first named type, else
/// its first primitive type.
#[must_use]
pub fn rust_impl_owner(fields: &mut FieldCursor<'_>, impl_item: NodeId) -> Option<NodeId> {
    let parsed = fields.parsed();
    let implemented = field_child(fields, impl_item, "type")?;
    RUST_OWNER_KINDS
        .iter()
        .find_map(|kind| parsed.first_of_kind(implemented, &parsed.language().kinds(&[kind])))
}

impl NameRule {
    pub(crate) fn name(self, fields: &mut FieldCursor<'_>, node: NodeId) -> Option<NodeId> {
        let parsed = fields.parsed();
        let definition = parsed.kind_name(node) == "function_definition";
        match self {
            NameRule::CDeclarator if definition => declarator_name(fields, node, &C_NAMES),
            NameRule::CppDeclarator if definition => declarator_name(fields, node, &CPP_NAMES),
            NameRule::RustImpl if parsed.kind_name(node) == "impl_item" => {
                rust_impl_owner(fields, node)
            }
            _ => field_child(fields, node, "name"),
        }
    }
}

impl BindingRule {
    pub(crate) fn name(self, fields: &mut FieldCursor<'_>, node: NodeId) -> Option<NodeId> {
        match self {
            BindingRule::Javascript => ecmascript_binding(fields, node, &JAVASCRIPT_BINDINGS),
            BindingRule::Typescript => ecmascript_binding(fields, node, &TYPESCRIPT_BINDINGS),
            BindingRule::Lua => lua_binding(fields.parsed(), node),
        }
    }
}

impl OwnerRule {
    pub(crate) fn owner(self, fields: &mut FieldCursor<'_>, node: NodeId) -> Option<NodeId> {
        let parsed = fields.parsed();
        match self {
            OwnerRule::GoReceiver => {
                let receiver = field_child(fields, node, "receiver")?;
                parsed.first_of_kind(receiver, &parsed.language().kinds(&["type_identifier"]))
            }
        }
    }
}

impl TypeChecking {
    /// Read the names that the file's imports bind to `typing` and to `typing.TYPE_CHECKING`.
    #[must_use]
    pub fn of(parsed: &Parsed) -> TypeChecking {
        let Some(fields) = ImportFields::of(parsed.language()) else {
            return TypeChecking::default();
        };
        let aliases = parsed
            .descendants(parsed.root())
            .flat_map(|node| fields.aliases(parsed, node))
            .collect();
        TypeChecking { aliases }
    }

    /// Return whether `condition`, the source text of an `if` condition, tests `TYPE_CHECKING`.
    #[must_use]
    pub fn tests(&self, condition: &str) -> bool {
        TYPE_CHECKING.contains(&condition) || self.aliases.iter().any(|alias| alias == condition)
    }
}

impl ImportFields {
    fn of(language: Language) -> Option<ImportFields> {
        Some(ImportFields {
            name: language.field("name")?,
            alias: language.field("alias")?,
            module: language.field("module_name")?,
        })
    }

    fn aliases(&self, parsed: &Parsed, node: NodeId) -> Vec<String> {
        match parsed.kind_name(node) {
            "import_statement" => self
                .renamed(parsed, node, TYPING)
                .map(|local| format!("{local}.{TYPE_CHECKING_NAME}"))
                .collect(),
            "import_from_statement" if self.names_typing(parsed, node) => self
                .renamed(parsed, node, TYPE_CHECKING_NAME)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }

    // A relative `from .typing import ...` names a module of the repository, not `typing`.
    fn names_typing(&self, parsed: &Parsed, node: NodeId) -> bool {
        parsed
            .children_by_field(node, self.module)
            .next()
            .is_some_and(|module| {
                parsed.kind_name(module) == "dotted_name" && parsed.text(module) == TYPING
            })
    }

    // The local name of each `imported as local` that the statement holds.
    fn renamed<'p>(
        &self,
        parsed: &'p Parsed,
        node: NodeId,
        imported: &'p str,
    ) -> impl Iterator<Item = &'p str> + 'p {
        let (name, alias) = (self.name, self.alias);
        parsed
            .children(node)
            .filter(|child| parsed.kind_name(*child) == "aliased_import")
            .filter(move |child| {
                let named = parsed.children_by_field(*child, name).next();
                named.is_some_and(|named| parsed.text(named) == imported)
            })
            .filter_map(move |child| parsed.children_by_field(child, alias).next())
            .map(|local| parsed.text(local))
    }
}

impl<'p> Skipped<'p> {
    pub(crate) fn read(rule: Option<SkipRule>, parsed: &'p Parsed) -> Skipped<'p> {
        let type_checking = rule.map(|rule| match rule {
            SkipRule::TypeChecking => TypeChecking::of(parsed),
        });
        Skipped {
            parsed,
            type_checking,
        }
    }

    /// Return whether the clone walk skips `node`.
    #[must_use]
    pub fn contains(&self, node: NodeId) -> bool {
        let Some(type_checking) = &self.type_checking else {
            return false;
        };
        // The clone walk visits the last sibling first, and a FieldCursor reaches each sibling
        // from the first one, so this test reads the field from the root.
        let parsed = self.parsed;
        parsed
            .language()
            .field("condition")
            .and_then(|condition| parsed.child_by_field(node, condition))
            .is_some_and(|condition| type_checking.tests(parsed.text(condition)))
    }
}

impl Cond {
    pub(crate) fn holds(self, fields: &mut FieldCursor<'_>, node: NodeId) -> bool {
        let parsed = fields.parsed();
        match self {
            Cond::HasNamedChild => parsed.named_child_count(node) > 0,
            Cond::NotDefaultCase => !parsed
                .children(node)
                .any(|child| !parsed.is_named(child) && parsed.kind_name(child) == "default"),
            Cond::CountedRustArm => field_child(fields, node, "pattern")
                .is_none_or(|pattern| !only_wildcard(parsed, pattern)),
            Cond::HasCondition => field_child(fields, node, "condition").is_some(),
            Cond::CountedRubyArm => counted_ruby_arm(fields, node),
            Cond::CountedCaseItem => !only_star(parsed, node),
        }
    }
}

fn field_child(fields: &mut FieldCursor<'_>, node: NodeId, name: &str) -> Option<NodeId> {
    let field = fields.parsed().language().field(name)?;
    fields.child_by_field(node, field)
}

fn declarator_name(fields: &mut FieldCursor<'_>, node: NodeId, names: &[&str]) -> Option<NodeId> {
    let parsed = fields.parsed();
    let mut current = field_child(fields, node, "declarator");
    while let Some(found) = current {
        let kind = parsed.kind_name(found);
        if names.contains(&kind) {
            return Some(found);
        }
        current = field_child(fields, found, "declarator").or_else(|| {
            DECLARATOR_WRAPPERS
                .contains(&kind)
                .then(|| parsed.named_children(found).next())
                .flatten()
        });
    }
    None
}

fn ecmascript_binding(
    fields: &mut FieldCursor<'_>,
    node: NodeId,
    bindings: &[(&str, &str)],
) -> Option<NodeId> {
    let parsed = fields.parsed();
    let parent = parsed.parent(node)?;
    let kind = parsed.kind_name(parent);
    let (_, field) = bindings.iter().find(|(binding, _)| *binding == kind)?;
    if field_child(fields, parent, "value") != Some(node) {
        return None;
    }
    let name = field_child(fields, parent, field)?;
    ECMASCRIPT_BINDING_NAMES
        .contains(&parsed.kind_name(name))
        .then_some(name)
}

fn lua_binding(parsed: &Parsed, node: NodeId) -> Option<NodeId> {
    let values = parsed.parent(node)?;
    if parsed.kind_name(values) != "expression_list" {
        return None;
    }
    let value = parsed.language().field("value")?;
    let first = parsed.children_by_field(values, value).next()?;
    let statement = parsed.parent(values)?;
    if first != node || parsed.kind_name(statement) != "assignment_statement" {
        return None;
    }
    lua_single_target(parsed, statement)
}

fn lua_single_target(parsed: &Parsed, statement: NodeId) -> Option<NodeId> {
    let targets = parsed
        .children(statement)
        .find(|child| parsed.kind_name(*child) == "variable_list")?;
    let field = parsed.language().field("name")?;
    let mut targets = parsed.children_by_field(targets, field);
    let (first, second) = (targets.next()?, targets.next());
    let binds = LUA_BINDING_NAMES.contains(&parsed.kind_name(first));
    (second.is_none() && binds).then_some(first)
}

fn only_wildcard(parsed: &Parsed, pattern: NodeId) -> bool {
    let mut children = parsed.children(pattern);
    let first = children.next();
    first.is_some_and(|child| parsed.kind_name(child) == "_") && children.next().is_none()
}

fn counted_ruby_arm(fields: &mut FieldCursor<'_>, node: NodeId) -> bool {
    let parsed = fields.parsed();
    if field_child(fields, node, "guard").is_some() {
        return true;
    }
    field_child(fields, node, "pattern").is_none_or(|pattern| {
        !(parsed.kind_name(pattern) == "identifier" && parsed.text(pattern) == "_")
    })
}

fn only_star(parsed: &Parsed, node: NodeId) -> bool {
    let Some(field) = parsed.language().field("value") else {
        return false;
    };
    let mut values = parsed.children_by_field(node, field);
    let first = values.next();
    first.is_some_and(|only| parsed.text(only) == "*") && values.next().is_none()
}
