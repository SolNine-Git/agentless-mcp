mod c_declarators;
mod c_includes;
mod data_languages;
mod generic_imports;
mod generic_walker;
mod identifier_refs;
mod lua_assignments;
mod python_definitions;
mod python_imports;
mod python_scopes;
mod rationales;
mod require_and_source;
mod rust_items;
mod rust_uses;
mod tree;

use syntax::{Extraction, ImportRule, NodeId, Parsed, SymbolRule};

use crate::spelling::normalise_signature;
use crate::stable_id::disambiguate;
use crate::values::{Facts, Import, Symbol, SymbolKind};
use tree::Tree;

/// Return the symbols, imports, identifier occurrences and error spans of one parse.
#[must_use]
pub fn extract(parsed: &Parsed) -> Facts {
    let tree = Tree::new(parsed);
    let extraction = parsed.language().extraction();
    let symbols = disambiguate(symbols(&tree, extraction));
    Facts {
        symbols: rationales::attached(symbols, rationales::rationales(&tree, extraction)),
        imports: imports(&tree, extraction),
        refs: identifier_refs::refs(&tree, extraction),
        errors: tree.error_spans(),
    }
}

fn symbols(tree: &Tree, extraction: &Extraction) -> Vec<Symbol> {
    match extraction.symbols {
        SymbolRule::Generic => generic_walker::symbols(tree, extraction),
        SymbolRule::Python => python_definitions::symbols(tree),
        SymbolRule::Rust => rust_items::symbols(tree),
        SymbolRule::CDeclarators => c_declarators::symbols(tree),
        SymbolRule::Lua => lua_assignments::symbols(tree),
        SymbolRule::DataKeys => data_languages::keys(tree),
        SymbolRule::Hcl => data_languages::hcl(tree),
        SymbolRule::Sql => data_languages::sql(tree),
    }
}

fn imports(tree: &Tree, extraction: &Extraction) -> Vec<Import> {
    match extraction.imports {
        ImportRule::Generic => generic_imports::imports(tree, &extraction.declarations),
        ImportRule::Python => python_imports::imports(tree),
        ImportRule::Rust => rust_uses::imports(tree),
        ImportRule::Include => c_includes::imports(tree),
        ImportRule::Require => require_and_source::requires(tree),
        ImportRule::Source => require_and_source::sources(tree),
        ImportRule::None => Vec::new(),
    }
}

impl Symbol {
    // A public symbol that spans `node`, with no signature, owner, decorators or bases yet.
    fn at(tree: &Tree, node: NodeId, name: &str, kind: SymbolKind) -> Symbol {
        Symbol {
            name: name.to_owned(),
            kind,
            line: tree.line(node),
            end_line: tree.end_line(node),
            signature: String::new(),
            parent: String::new(),
            decorated: false,
            bases: Vec::new(),
            is_public: true,
            rationales: Vec::new(),
            duplicate_index: 0,
        }
    }

    fn signed(mut self, signature: &str) -> Symbol {
        self.signature = normalise_signature(signature);
        self
    }

    fn owned_by(mut self, parent: &str) -> Symbol {
        parent.clone_into(&mut self.parent);
        self
    }

    fn public(mut self, is_public: bool) -> Symbol {
        self.is_public = is_public;
        self
    }

    fn decorated(mut self, decorated: bool) -> Symbol {
        self.decorated = decorated;
        self
    }

    fn based_on(mut self, bases: Vec<String>) -> Symbol {
        self.bases = bases;
        self
    }

    fn of_kind(mut self, kind: SymbolKind) -> Symbol {
        self.kind = kind;
        self
    }
}

impl Import {
    // A statement on the line of `node` that names nothing and binds a module object.
    fn at(tree: &Tree, node: NodeId, module: &str, is_relative: bool) -> Import {
        Import {
            module: module.to_owned(),
            names: Vec::new(),
            local_names: Vec::new(),
            is_relative,
            relative_level: 0,
            line: tree.line(node),
            binds_all: false,
            alias: String::new(),
        }
    }

    // Name the members and the local names they bind; equal lists keep the local names empty.
    fn naming(mut self, names: Vec<String>, local_names: Vec<String>) -> Import {
        self.local_names = if local_names == names {
            Vec::new()
        } else {
            local_names
        };
        self.names = names;
        self
    }
}

// The underscore convention: a name that starts with `_` is not public.
fn conventionally_public(name: &str) -> bool {
    !name.starts_with('_')
}

// An `async` keyword among the declaration's own children, or in its Rust function modifiers.
fn declares_async(tree: &Tree, node: NodeId) -> bool {
    tree.children(node).any(|child| {
        tree.is(child, "async")
            || (tree.is(child, "function_modifiers")
                && tree
                    .children(child)
                    .any(|modifier| tree.is(modifier, "async")))
    })
}
