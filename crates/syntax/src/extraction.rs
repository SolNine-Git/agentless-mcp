use std::sync::OnceLock;

use crate::tables::{DeclarationTable, Names};
use crate::{FieldId, KindSet, Language};

/// The walker that emits a language's symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolRule {
    Generic,
    Python,
    Rust,
    CDeclarators,
    Lua,
    DataKeys,
    Hcl,
    Sql,
}

/// The walker that emits a language's imports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportRule {
    Generic,
    Python,
    Rust,
    Include,
    Require,
    Source,
    None,
}

/// How the reference pass gives each identifier occurrence its role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleRule {
    Plain,
    Python,
}

/// How one import statement binds names into the importing file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportBinding {
    ModuleObject,
    Specifiers,
    LastSegment,
}

/// The extraction tables of one language, with kind and field names resolved against its grammar.
pub struct Extraction {
    /// The language's stable-id prefix, such as `py`.
    pub prefix: &'static str,
    pub symbols: SymbolRule,
    pub imports: ImportRule,
    pub roles: RoleRule,
    /// The kinds whose nodes name something, the raw material of the reference pass.
    pub identifiers: KindSet,
    pub comments: KindSet,
    /// The kinds that are a statement block: the body of a function.
    pub bodies: KindSet,
    /// The function kinds that the skeleton elides beyond the declared function kinds.
    pub skeleton_functions: KindSet,
    pub declarations: Declarations,
    builtins: Names,
}

/// The declaration kinds and fields of one language; every set is empty for a language with none.
pub struct Declarations {
    pub functions: KindSet,
    pub classes: KindSet,
    pub imports: KindSet,
    pub name_field: Option<FieldId>,
    /// The kinds of the child that names a declaration when the name field finds none.
    pub names: KindSet,
    pub class_bodies: KindSet,
    pub signature_from_header: bool,
    pub receiver_field: Option<FieldId>,
    pub import_path_field: Option<FieldId>,
    pub import_path_kind: KindSet,
    pub import_binding: ImportBinding,
    pub modifiers: KindSet,
    /// Whether visibility comes from modifier keywords: the table lists modifier kinds, present in
    /// the grammar or not.
    pub keyword_visibility: bool,
    pub bindings: KindSet,
    pub function_values: KindSet,
    pub constants: KindSet,
    pub constant_keywords: &'static [&'static str],
}

const NO_DECLARATIONS: DeclarationTable = DeclarationTable {
    functions: &[],
    classes: &[],
    imports: &[],
    name_field: None,
    import_path_field: None,
    import_path_kind: "",
    names: &[],
    class_bodies: &[],
    signature_from_header: false,
    receiver_field: None,
    import_binding: ImportBinding::ModuleObject,
    modifiers: &[],
    bindings: &[],
    function_values: &[],
    constants: &[],
    constant_keywords: &[],
};

static EXTRACTIONS: [OnceLock<Extraction>; 22] = [const { OnceLock::new() }; 22];

pub(crate) fn extraction(language: Language) -> &'static Extraction {
    EXTRACTIONS[language as usize].get_or_init(|| resolve(language))
}

fn resolve(language: Language) -> Extraction {
    let table = language.table();
    let extraction = table.extraction;
    let declarations = extraction.declarations.unwrap_or(&NO_DECLARATIONS);
    Extraction {
        prefix: extraction.prefix,
        symbols: extraction.symbols,
        imports: extraction.imports,
        roles: extraction.roles,
        identifiers: language.kinds(extraction.identifiers),
        comments: language.kinds(table.comments),
        bodies: language.kinds(extraction.bodies),
        skeleton_functions: language.kinds(extraction.skeleton_functions),
        declarations: Declarations::resolve(language, declarations),
        builtins: extraction.builtins,
    }
}

impl Extraction {
    /// Return whether `name` is one of the names that the language binds in every scope.
    #[must_use]
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtins.binary_search(&name).is_ok()
    }
}

impl Declarations {
    fn resolve(language: Language, table: &DeclarationTable) -> Declarations {
        let field = |name: Option<&str>| name.and_then(|name| language.field(name));
        Declarations {
            functions: language.kinds(table.functions),
            classes: language.kinds(table.classes),
            imports: language.kinds(table.imports),
            name_field: field(table.name_field),
            names: language.kinds(table.names),
            class_bodies: language.kinds(table.class_bodies),
            signature_from_header: table.signature_from_header,
            receiver_field: field(table.receiver_field),
            import_path_field: field(table.import_path_field),
            import_path_kind: language.kinds(&[table.import_path_kind]),
            import_binding: table.import_binding,
            modifiers: language.kinds(table.modifiers),
            keyword_visibility: !table.modifiers.is_empty(),
            bindings: language.kinds(table.bindings),
            function_values: language.kinds(table.function_values),
            constants: language.kinds(table.constants),
            constant_keywords: table.constant_keywords,
        }
    }
}
