//! The Python roles and scopes: which identifiers bind, declare, import or reference a name, and
//! which lexical scope a name at a node resolves in.

use std::collections::{HashMap, HashSet};

use syntax::NodeId;

use super::tree::{Field, Tree};
use crate::pattern::upper_case;
use crate::values::Role;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Function,
    Class,
    Comprehension,
}

const SCOPE_KINDS: [(&str, ScopeKind); 7] = [
    ("function_definition", ScopeKind::Function),
    ("lambda", ScopeKind::Function),
    ("class_definition", ScopeKind::Class),
    ("list_comprehension", ScopeKind::Comprehension),
    ("set_comprehension", ScopeKind::Comprehension),
    ("dictionary_comprehension", ScopeKind::Comprehension),
    ("generator_expression", ScopeKind::Comprehension),
];
const IMPORT_STATEMENTS: [&str; 2] = ["import_statement", "import_from_statement"];
const DEFINITIONS: [&str; 2] = ["function_definition", "class_definition"];
const PARAMETER_OWNERS: [&str; 2] = ["function_definition", "lambda"];
const NAMED_PARAMETERS: [&str; 2] = ["default_parameter", "typed_default_parameter"];
const SPLAT_PATTERNS: [&str; 2] = ["list_splat_pattern", "dictionary_splat_pattern"];
const BINDING_TARGETS: [(&str, Field); 6] = [
    ("assignment", Field::Left),
    ("augmented_assignment", Field::Left),
    ("named_expression", Field::Name),
    ("for_statement", Field::Left),
    ("for_in_clause", Field::Left),
    ("as_pattern", Field::Alias),
];
const SCOPE_STATEMENTS: [&str; 2] = ["global_statement", "nonlocal_statement"];

// The roles that the syntax settles, the module qualifiers of module attributes, and the scopes
// that answer every other identifier.
pub(super) struct Roles {
    direct: Vec<Option<Role>>,
    qualifiers: HashMap<NodeId, String>,
    scopes: Scopes,
}

struct Scope {
    kind: ScopeKind,
    bindings: HashSet<String>,
    globals: HashSet<String>,
    imports: HashSet<String>,
}

// The scopes of one tree, each found by the node that opens it and by the node that bounds it.
struct Scopes {
    records: Vec<Scope>,
    by_node: HashMap<NodeId, usize>,
    by_boundary: HashMap<NodeId, usize>,
    // For each node, the innermost boundary at or above it; for each boundary, the next one out.
    innermost: Vec<Option<NodeId>>,
    outer: HashMap<NodeId, Option<NodeId>>,
}

pub(super) fn roles(tree: &Tree) -> Roles {
    let mut scopes = Scopes::build(tree);
    let mut roles = Roles {
        direct: vec![None; tree.parsed.node_count() as usize],
        qualifiers: HashMap::new(),
        scopes: Scopes::empty(),
    };
    for node in tree.preorder(tree.root()) {
        if tree.is_any(node, &IMPORT_STATEMENTS) {
            roles.mark_import(tree, &mut scopes, node);
        }
    }
    for node in tree.preorder(tree.root()) {
        roles.bind(tree, &mut scopes, node);
    }
    for node in tree.preorder(tree.root()) {
        roles.mark_syntax(tree, &scopes, node);
    }
    roles.scopes = scopes;
    roles
}

impl Roles {
    // The role the syntax gave `node`, else the role of the nearest scope that holds `name`.
    pub(super) fn role(&self, tree: &Tree, node: NodeId, name: &str) -> Role {
        if let Some(role) = self.direct[node.index()] {
            return role;
        }
        for scope in self.scopes.visible(tree, node) {
            if scope.globals.contains(name) {
                return Role::Reference;
            }
            if scope.bindings.contains(name) {
                return Role::Local;
            }
        }
        if tree.parsed.language().extraction().is_builtin(name) {
            Role::Builtin
        } else {
            Role::Reference
        }
    }

    pub(super) fn qualifier(&self, node: NodeId) -> &str {
        self.qualifiers.get(&node).map_or("", String::as_str)
    }

    fn set_default(&mut self, node: NodeId, role: Role) {
        let slot = &mut self.direct[node.index()];
        if slot.is_none() {
            *slot = Some(role);
        }
    }

    fn mark_all(&mut self, nodes: &[NodeId], role: Role) {
        for node in nodes {
            self.set_default(*node, role);
        }
    }

    fn mark_import(&mut self, tree: &Tree, scopes: &mut Scopes, node: NodeId) {
        self.mark_all(&descendant_identifiers(tree, node), Role::Import);
        let names = imported_module_names(tree, node);
        let scope = scopes.nearest(tree, node);
        scopes.records[scope]
            .imports
            .extend(names.into_iter().map(str::to_owned));
    }

    fn bind(&mut self, tree: &Tree, scopes: &mut Scopes, node: NodeId) {
        bind_declaration(tree, scopes, node);
        self.bind_parameters(tree, scopes, node);
        self.bind_target(tree, scopes, node);
        self.bind_scope_statement(tree, scopes, node);
    }

    fn bind_parameters(&mut self, tree: &Tree, scopes: &mut Scopes, node: NodeId) {
        if !tree.is_any(node, &PARAMETER_OWNERS) {
            return;
        }
        let Some(parameters) = tree.field(node, Field::Parameters) else {
            return;
        };
        let bound = parameter_nodes(tree, parameters);
        self.mark_all(&bound, Role::Binding);
        let scope = scopes.by_node[&node];
        let names = bound.iter().map(|name| tree.text(*name).to_owned());
        scopes.records[scope].bindings.extend(names);
    }

    fn bind_target(&mut self, tree: &Tree, scopes: &mut Scopes, node: NodeId) {
        let Some(target) = binding_target(tree, node) else {
            return;
        };
        let bound = binding_identifiers(tree, target);
        self.mark_all(&bound, Role::Binding);
        let scope = scopes.nearest(tree, node);
        let record = &mut scopes.records[scope];
        let module = record.kind == ScopeKind::Module;
        // A module-level upper-case name is a constant, which a reference reaches as a symbol.
        let names = bound
            .iter()
            .map(|name| tree.text(*name))
            .filter(|name| !(module && upper_case(name)));
        record.bindings.extend(names.map(str::to_owned));
    }

    fn bind_scope_statement(&mut self, tree: &Tree, scopes: &mut Scopes, node: NodeId) {
        if !tree.is_any(node, &SCOPE_STATEMENTS) {
            return;
        }
        let declared = descendant_identifiers(tree, node);
        self.mark_all(&declared, Role::Binding);
        let scope = scopes.nearest(tree, node);
        let record = &mut scopes.records[scope];
        let names = declared.iter().map(|name| tree.text(*name).to_owned());
        if tree.is(node, "global_statement") {
            record.globals.extend(names);
        } else {
            record.bindings.extend(names);
        }
    }

    fn mark_syntax(&mut self, tree: &Tree, scopes: &Scopes, node: NodeId) {
        if tree.is(node, "attribute") {
            self.mark_attribute(tree, scopes, node);
        }
        if tree.is(node, "keyword_argument") {
            self.mark_field(tree, node, Role::Keyword);
        }
        if tree.is_any(node, &DEFINITIONS) {
            self.mark_field(tree, node, Role::Declaration);
        }
    }

    fn mark_field(&mut self, tree: &Tree, node: NodeId, role: Role) {
        if let Some(name) = tree.field(node, Field::Name) {
            self.set_default(name, role);
        }
    }

    // `a.b`: `b` is a module attribute when `a` names a module this scope imports, and `a` is
    // then its qualifier; else `b` is a plain attribute.
    fn mark_attribute(&mut self, tree: &Tree, scopes: &Scopes, node: NodeId) {
        let Some(attribute) = tree.field(node, Field::Attribute) else {
            return;
        };
        let object = tree
            .field(node, Field::Object)
            .filter(|object| tree.is(*object, "identifier"));
        let qualifier = object.map(|object| (object, tree.text(object)));
        let imported = qualifier.filter(|(_, name)| scopes.binds_import(tree, name, node));
        let Some((object, name)) = imported else {
            self.set_default(attribute, Role::Attribute);
            return;
        };
        self.set_default(attribute, Role::ModuleAttribute);
        self.set_default(object, Role::ModuleQualifier);
        self.qualifiers.insert(attribute, name.to_owned());
    }
}

fn bind_declaration(tree: &Tree, scopes: &mut Scopes, node: NodeId) {
    if !tree.is_any(node, &DEFINITIONS) {
        return;
    }
    let Some(name) = tree.field(node, Field::Name) else {
        return;
    };
    let scope = scopes.nearest(tree, name);
    let record = &mut scopes.records[scope];
    if record.kind != ScopeKind::Module {
        record.bindings.insert(tree.text(name).to_owned());
    }
}

impl Scope {
    fn new(kind: ScopeKind) -> Scope {
        Scope {
            kind,
            bindings: HashSet::new(),
            globals: HashSet::new(),
            imports: HashSet::new(),
        }
    }
}

impl Scopes {
    fn empty() -> Scopes {
        Scopes {
            records: Vec::new(),
            by_node: HashMap::new(),
            by_boundary: HashMap::new(),
            innermost: Vec::new(),
            outer: HashMap::new(),
        }
    }

    // The module scope, then one scope per scope node in document order. A function's or a
    // class's names apply in its body; the first scope to claim a boundary keeps it.
    fn build(tree: &Tree) -> Scopes {
        let root = tree.root();
        let mut scopes = Scopes::empty();
        scopes.add(root, root, ScopeKind::Module);
        for node in tree.preorder(root) {
            let Some(kind) = scope_kind(tree, node) else {
                continue;
            };
            let body = matches!(kind, ScopeKind::Function | ScopeKind::Class)
                .then(|| tree.field(node, Field::Body))
                .flatten();
            scopes.add(node, body.unwrap_or(node), kind);
        }
        scopes.index_boundaries(tree);
        scopes
    }

    fn add(&mut self, node: NodeId, boundary: NodeId, kind: ScopeKind) {
        let index = self.records.len();
        self.records.push(Scope::new(kind));
        self.by_node.insert(node, index);
        self.by_boundary.entry(boundary).or_insert(index);
    }

    // One downward pass, so that a query costs the scopes around a node and not its depth.
    fn index_boundaries(&mut self, tree: &Tree) {
        self.innermost = vec![None; tree.parsed.node_count() as usize];
        for node in tree.preorder(tree.root()) {
            let inherited = tree
                .parsed
                .parent(node)
                .and_then(|parent| self.innermost[parent.index()]);
            if self.by_boundary.contains_key(&node) {
                self.outer.insert(node, inherited);
                self.innermost[node.index()] = Some(node);
            } else {
                self.innermost[node.index()] = inherited;
            }
        }
    }

    // Each boundary at or above `node`, innermost first.
    fn boundaries(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.innermost[node.index()], |boundary| {
            self.outer.get(boundary).copied().flatten()
        })
    }

    // The scope that `node` binds its names in; `:=` steps out of every comprehension.
    fn nearest(&self, tree: &Tree, node: NodeId) -> usize {
        let escapes = tree.is(node, "named_expression");
        self.boundaries(node)
            .filter_map(|boundary| self.by_boundary.get(&boundary).copied())
            .find(|scope| !(escapes && self.records[*scope].kind == ScopeKind::Comprehension))
            .expect("the module scope bounds the root, which encloses every node")
    }

    // Each scope around `node`, innermost first, without a comprehension whose first iterable
    // holds `node`: that iterable is evaluated outside the comprehension.
    fn enclosing<'s>(&'s self, tree: &'s Tree, node: NodeId) -> impl Iterator<Item = &'s Scope> {
        self.boundaries(node).filter_map(move |boundary| {
            let scope = &self.records[*self.by_boundary.get(&boundary)?];
            let outside =
                scope.kind == ScopeKind::Comprehension && in_first_iterable(tree, node, boundary);
            (!outside).then_some(scope)
        })
    }

    // The scopes a name at `node` resolves in: a class body is no closure scope for a function or
    // a comprehension inside it.
    fn visible<'s>(&'s self, tree: &'s Tree, node: NodeId) -> impl Iterator<Item = &'s Scope> {
        let mut nested = false;
        self.enclosing(tree, node).filter(move |scope| {
            if matches!(scope.kind, ScopeKind::Function | ScopeKind::Comprehension) {
                nested = true;
            }
            !(scope.kind == ScopeKind::Class && nested)
        })
    }

    // Whether `name` at `node` is a module object that a visible scope imports.
    fn binds_import(&self, tree: &Tree, name: &str, node: NodeId) -> bool {
        for scope in self.visible(tree, node) {
            if scope.globals.contains(name) {
                continue;
            }
            if scope.bindings.contains(name) {
                return false;
            }
            if scope.imports.contains(name) {
                return true;
            }
        }
        false
    }
}

fn scope_kind(tree: &Tree, node: NodeId) -> Option<ScopeKind> {
    let kind = tree.kind(node);
    SCOPE_KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, scope)| *scope)
}

fn in_first_iterable(tree: &Tree, node: NodeId, comprehension: NodeId) -> bool {
    let clause = tree
        .named_children(comprehension)
        .find(|child| tree.is(*child, "for_in_clause"));
    let Some(iterable) = clause.and_then(|clause| tree.field(clause, Field::Right)) else {
        return false;
    };
    let (iterable, node) = (
        tree.parsed.byte_range(iterable),
        tree.parsed.byte_range(node),
    );
    iterable.start <= node.start && node.end <= iterable.end
}

// The local names one import statement introduces: an alias, the first segment of `import a.b`,
// or the last segment of `from pkg import mod`.
fn imported_module_names<'a>(tree: &Tree<'a>, node: NodeId) -> Vec<&'a str> {
    let Some(name) = tree.field_id(Field::Name) else {
        return Vec::new();
    };
    let bare = tree.is(node, "import_statement");
    tree.parsed
        .children_by_field(node, name)
        .filter_map(|child| {
            if let Some(alias) = tree.field(child, Field::Alias) {
                return Some(tree.text(alias));
            }
            let identifiers = descendant_identifiers(tree, child);
            let selected = if bare {
                identifiers.first()
            } else {
                identifiers.last()
            };
            selected.map(|selected| tree.text(*selected))
        })
        .collect()
}

// The names a parameter list binds, without its annotations and defaults.
fn parameter_nodes(tree: &Tree, parameters: NodeId) -> Vec<NodeId> {
    tree.named_children(parameters)
        .filter_map(|child| parameter_name(tree, child))
        .collect()
}

fn parameter_name(tree: &Tree, parameter: NodeId) -> Option<NodeId> {
    let mut target = Some(parameter);
    if tree.is_any(parameter, &NAMED_PARAMETERS) {
        target = tree.field(parameter, Field::Name);
    }
    if let Some(typed) = target.filter(|target| tree.is(*target, "typed_parameter")) {
        let annotation = tree.field(typed, Field::Type);
        target = tree
            .named_children(typed)
            .find(|part| Some(*part) != annotation);
    }
    if let Some(splat) = target.filter(|target| tree.is_any(*target, &SPLAT_PATTERNS)) {
        target = tree.named_children(splat).next();
    }
    target.filter(|target| tree.is(*target, "identifier"))
}

fn binding_target(tree: &Tree, node: NodeId) -> Option<NodeId> {
    let kind = tree.kind(node);
    let (_, field) = BINDING_TARGETS.iter().find(|(name, _)| *name == kind)?;
    tree.field(node, *field)
}

// The identifiers a target binds, in document order, without the parts of an attribute or a
// subscript.
fn binding_identifiers(tree: &Tree, target: NodeId) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut pending = vec![target];
    while let Some(node) = pending.pop() {
        if tree.is_any(node, &["attribute", "subscript"]) {
            continue;
        }
        if tree.is(node, "identifier") {
            found.push(node);
            continue;
        }
        let children: Vec<NodeId> = tree.named_children(node).collect();
        pending.extend(children.into_iter().rev());
    }
    found
}

fn descendant_identifiers(tree: &Tree, node: NodeId) -> Vec<NodeId> {
    tree.preorder(node)
        .filter(|child| tree.is(*child, "identifier"))
        .collect()
}
