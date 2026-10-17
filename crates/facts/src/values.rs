use kernel::Span;
use serde::{Deserialize, Serialize};

/// The symbols, imports, identifier occurrences and error spans of one parsed file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// The symbols in the order the extraction emits them.
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    /// Every identifier occurrence, in document order.
    pub refs: Vec<Ref>,
    /// The byte ranges of the outermost ERROR nodes in document order; MISSING nodes are not here.
    pub errors: Vec<Span>,
}

/// One declaration of a file: a function, a class, a constant or a key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// The 1-based first line.
    pub line: u32,
    /// The 1-based last line.
    pub end_line: u32,
    /// One line of at most 80 code points.
    pub signature: String,
    /// The owner that qualifies the name, or empty.
    pub parent: String,
    /// Whether one or more decorators wrap the declaration. Only Python declarations have them.
    pub decorated: bool,
    /// The superclass texts as the source spells them.
    pub bases: Vec<String>,
    pub is_public: bool,
    pub rationales: Vec<Rationale>,
    /// How many earlier symbols of the file share the qualified name.
    pub duplicate_index: u32,
}

/// What a symbol declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Class,
    Function,
    Method,
    Constant,
    TypeAlias,
    Protocol,
    Dataclass,
    Enum,
    Decorator,
}

/// One design-rationale comment, attached to the symbol that contains it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rationale {
    pub kind: RationaleKind,
    pub text: String,
    /// The 1-based line of the marker or citation.
    pub line: u32,
    pub citations: Vec<String>,
    /// How many earlier rationales of the symbol sit on the same line.
    pub duplicate_index: u32,
}

/// The marker that makes a comment line a rationale, or a citation with no marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RationaleKind {
    Note,
    Why,
    Hack,
    Todo,
    Citation,
}

/// One import statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    pub module: String,
    /// The members the statement names.
    pub names: Vec<String>,
    /// The local name of each member, by position; empty when each binds under its own name.
    pub local_names: Vec<String>,
    pub is_relative: bool,
    pub relative_level: u32,
    /// The 1-based line of the statement.
    pub line: u32,
    /// Whether the statement binds every name that the target defines.
    pub binds_all: bool,
    /// The local name that a module-object import binds, or empty when the language chose it.
    pub alias: String,
}

/// One identifier occurrence: the name, its line, and what it does there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    pub name: String,
    /// The 1-based line of the occurrence.
    pub line: u32,
    pub role: Role,
    /// The imported module name that qualifies a module attribute, or empty.
    pub qualifier: String,
}

/// The syntactic meaning of one identifier occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Reference,
    Local,
    Binding,
    Declaration,
    Import,
    Attribute,
    ModuleQualifier,
    ModuleAttribute,
    Keyword,
    Builtin,
}

const SYMBOL_KINDS: [&str; 9] = [
    "class",
    "function",
    "method",
    "constant",
    "type_alias",
    "protocol",
    "dataclass",
    "enum",
    "decorator",
];
const RATIONALE_KINDS: [&str; 5] = ["note", "why", "hack", "todo", "citation"];
const ROLES: [&str; 10] = [
    "reference",
    "local",
    "binding",
    "declaration",
    "import",
    "attribute",
    "module_qualifier",
    "module_attribute",
    "keyword",
    "builtin",
];

impl Symbol {
    /// Return `parent.name` for a symbol with an owner, else the name.
    #[must_use]
    pub fn qualname(&self) -> String {
        qualified(&self.parent, &self.name)
    }
}

pub(crate) fn qualified(owner: &str, name: &str) -> String {
    if owner.is_empty() {
        name.to_owned()
    } else {
        format!("{owner}.{name}")
    }
}

impl SymbolKind {
    /// Every kind, in declaration order.
    pub const ALL: [SymbolKind; 9] = [
        SymbolKind::Class,
        SymbolKind::Function,
        SymbolKind::Method,
        SymbolKind::Constant,
        SymbolKind::TypeAlias,
        SymbolKind::Protocol,
        SymbolKind::Dataclass,
        SymbolKind::Enum,
        SymbolKind::Decorator,
    ];

    /// Return the kind's name, such as `type_alias`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        SYMBOL_KINDS[self as usize]
    }

    /// Return the kind named `name`, compared case-sensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<SymbolKind> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

impl RationaleKind {
    /// Return the kind's name, such as `note`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        RATIONALE_KINDS[self as usize]
    }
}

impl Import {
    /// Return each member with the local name it binds, by position.
    pub fn bound_names(&self) -> impl Iterator<Item = (&str, &str)> {
        let locals = if self.local_names.is_empty() {
            &self.names
        } else {
            &self.local_names
        };
        self.names
            .iter()
            .zip(locals)
            .map(|(name, local)| (name.as_str(), local.as_str()))
    }
}

impl Ref {
    /// Return whether the occurrence is an unqualified reference to a repository symbol.
    #[must_use]
    pub fn is_reference(&self) -> bool {
        self.role == Role::Reference
    }

    /// Return whether the occurrence carries enough syntax to attempt a binding.
    #[must_use]
    pub fn is_resolvable(&self) -> bool {
        matches!(self.role, Role::Reference | Role::ModuleAttribute)
    }
}

impl Role {
    /// Return the role's name, such as `module_attribute`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        ROLES[self as usize]
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::{Import, RationaleKind, Role, SymbolKind};

    fn serde_name(value: impl Serialize) -> String {
        serde_json::to_value(value)
            .expect("an enum serializes")
            .as_str()
            .expect("a unit variant serializes as a string")
            .to_owned()
    }

    #[test]
    fn each_name_is_the_serde_name_and_reads_back() {
        for kind in SymbolKind::ALL {
            assert_eq!(serde_name(kind), kind.as_str());
            assert_eq!(SymbolKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(SymbolKind::from_name("Class"), None);
        let kinds = [
            RationaleKind::Note,
            RationaleKind::Why,
            RationaleKind::Hack,
            RationaleKind::Todo,
            RationaleKind::Citation,
        ];
        for kind in kinds {
            assert_eq!(serde_name(kind), kind.as_str());
        }
        let roles = [
            Role::Reference,
            Role::Local,
            Role::Binding,
            Role::Declaration,
            Role::Import,
            Role::Attribute,
            Role::ModuleQualifier,
            Role::ModuleAttribute,
            Role::Keyword,
            Role::Builtin,
        ];
        for role in roles {
            assert_eq!(serde_name(role), role.as_str());
        }
    }

    #[test]
    fn a_member_binds_its_local_name_or_its_own() {
        let mut import = Import {
            module: "pkg".to_owned(),
            names: vec!["a".to_owned(), "b".to_owned()],
            local_names: Vec::new(),
            is_relative: false,
            relative_level: 0,
            line: 1,
            binds_all: false,
            alias: String::new(),
        };
        let pairs: Vec<(&str, &str)> = import.bound_names().collect();
        assert_eq!(pairs, [("a", "a"), ("b", "b")]);
        import.local_names = vec!["x".to_owned(), "b".to_owned()];
        let pairs: Vec<(&str, &str)> = import.bound_names().collect();
        assert_eq!(pairs, [("a", "x"), ("b", "b")]);
    }
}
