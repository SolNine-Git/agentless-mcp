//! The Rust roles: the names that a pattern binds, each use of a visible local, member names, the
//! last segment of a type path, and the names of the items that are no symbol.

use std::collections::HashMap;
use std::ops::Range;

use syntax::{NodeId, ScopeKinds};

use super::tree::{Field, Tree};
use crate::Declared;
use crate::values::{Role, Symbol};

// The paths whose `name` is the last segment and whose `path` qualifies it.
pub(super) const RUST_PATHS: [&str; 2] = ["scoped_identifier", "scoped_type_identifier"];

// Where the names of a binding site are visible, after rust-analyzer's ExprScopes and rustc's
// ribs: a nested item sees no local of the code around it.
#[derive(Clone, Copy)]
enum Site {
    // The parameters of a function or a closure, in its body.
    Parameters,
    // The parameters of a signature or a function type, nowhere.
    Signature,
    // A `let` pattern, from the end of the statement to the end of its block.
    Let,
    // An `if let`, `while let` or guard pattern, from the end of the condition to the end of what
    // it guards.
    Condition,
    // A match arm's pattern, in its guard and its value.
    Arm,
    // A `for` pattern, in the loop body.
    For,
}

const SITES: [(&str, Site); 8] = [
    ("closure_expression", Site::Parameters),
    ("for_expression", Site::For),
    ("function_item", Site::Parameters),
    ("function_signature_item", Site::Signature),
    ("function_type", Site::Signature),
    ("let_condition", Site::Condition),
    ("let_declaration", Site::Let),
    ("match_arm", Site::Arm),
];
const IDENTIFIER: &str = "identifier";
const MEMBER: &str = "field_identifier";
const SHORTHAND: &str = "shorthand_field_identifier";
const FIELD_PATTERN: &str = "field_pattern";
const VARIANT: &str = "enum_variant";
const MACRO_CALL: &str = "macro_invocation";
const TOKENS: &str = "token_tree";
const IF: &str = "if_expression";
// The nodes between a `let` condition and the expression whose scope it opens.
const CONDITION_WRAPPERS: [&str; 3] = ["let_chain", "match_pattern", "parenthesized_expression"];
const TYPE_QUALIFIERS: [&str; 2] = ["bracketed_type", "generic_type"];
const NAMED_QUALIFIERS: [&str; 3] = ["identifier", "scoped_identifier", "type_identifier"];

// The role that the Rust syntax gives each node, where it gives one.
pub(super) struct RustRoles {
    roles: Vec<Option<Role>>,
}

impl RustRoles {
    pub(super) fn role(&self, node: NodeId) -> Option<Role> {
        self.roles[node.index()]
    }
}

// The innermost item around a node, and whether the node sits in an attribute or a macro
// definition, or in the arguments of a macro call.
#[derive(Clone, Copy, Default)]
struct Place {
    owner: Option<NodeId>,
    opaque: bool,
    in_macro: bool,
}

// A name that a pattern binds: visible in `visible`, to the code of the same owner.
struct Local {
    owner: Option<NodeId>,
    visible: Range<usize>,
}

struct Walk<'t, 'a> {
    tree: &'t Tree<'a>,
    kinds: &'t ScopeKinds,
    places: Vec<Place>,
    roles: Vec<Option<Role>>,
    in_pattern: Vec<bool>,
    locals: HashMap<&'a str, Vec<Local>>,
}

pub(super) fn roles(tree: &Tree, kinds: &ScopeKinds, symbols: &[Symbol]) -> RustRoles {
    let mut walk = Walk::new(tree, kinds);
    for node in tree.preorder(tree.root()) {
        if let Some(site) = tree.keyed(node, &SITES) {
            walk.bind_site(node, site);
        }
    }
    let declared = Declared::of(symbols, &[]);
    for node in tree.preorder(tree.root()) {
        walk.mark_syntax(node, &declared);
    }
    for node in tree.preorder(tree.root()) {
        walk.mark_local(node);
    }
    RustRoles { roles: walk.roles }
}

impl<'t, 'a> Walk<'t, 'a> {
    fn new(tree: &'t Tree<'a>, kinds: &'t ScopeKinds) -> Walk<'t, 'a> {
        let count = tree.parsed.node_count() as usize;
        Walk {
            tree,
            kinds,
            places: places(tree, kinds),
            roles: vec![None; count],
            in_pattern: vec![false; count],
            locals: HashMap::new(),
        }
    }

    fn bind_site(&mut self, node: NodeId, site: Site) {
        for (pattern, visible) in site_patterns(self.tree, node, site) {
            self.bind_pattern(pattern, &visible);
        }
    }

    // Every node of a pattern is pattern syntax, and each name it binds is a binding.
    fn bind_pattern(&mut self, pattern: NodeId, visible: &Range<usize>) {
        for node in self.tree.preorder(pattern) {
            self.in_pattern[node.index()] = true;
        }
        for name in pattern_names(self.tree, self.kinds, pattern) {
            if self.tree.is(name, IDENTIFIER) {
                self.roles[name.index()] = Some(Role::Binding);
            }
            if !visible.is_empty() {
                let local = Local {
                    owner: self.places[name.index()].owner,
                    visible: visible.clone(),
                };
                self.locals
                    .entry(self.tree.text(name))
                    .or_default()
                    .push(local);
            }
        }
    }

    // A declaration keeps its role in an attribute or a macro definition; the other roles do not
    // reach their tokens.
    fn mark_syntax(&mut self, node: NodeId, declared: &Declared) {
        let at = node.index();
        if self.roles[at].is_some() {
            return;
        }
        if self.names_item(node) {
            let symbol = declared.holds(self.tree.text(node), self.tree.line(node));
            self.roles[at] = (!symbol).then_some(Role::Declaration);
        } else if !self.places[at].opaque && self.names_member(node) {
            self.roles[at] = Some(Role::Attribute);
        }
    }

    fn mark_local(&mut self, node: NodeId) {
        let at = node.index();
        let place = self.places[at];
        let candidate = self.roles[at].is_none()
            && !self.in_pattern[at]
            && !place.opaque
            && self.tree.is(node, IDENTIFIER);
        if candidate && self.use_position(node, place) && self.visible_local(node, place) {
            self.roles[at] = Some(Role::Local);
        }
    }

    // The name of an item or of an enum variant.
    fn names_item(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let Some(parent) = tree.parsed.parent(node) else {
            return false;
        };
        let item = self.kinds.owners.contains(tree.parsed.kind(parent)) || tree.is(parent, VARIANT);
        item && tree.field(parent, Field::Name) == Some(node)
    }

    // A field or a method name, a token after `.` in a macro call's arguments, or the last
    // segment of a type path: each binds nothing, as a Python attribute does.
    fn names_member(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let in_macro = self.places[node.index()].in_macro;
        tree.is(node, MEMBER)
            || (in_macro && tree.is(node, IDENTIFIER) && self.token_before(node) == Some("."))
            || type_path_tail(tree, node)
    }

    // Not a path or use-tree segment, a macro name or an item name, and in a macro call's
    // arguments not a token of a path or a member.
    fn use_position(&self, node: NodeId, place: Place) -> bool {
        let tree = self.tree;
        let Some(parent) = tree.parsed.parent(node) else {
            return false;
        };
        if self.kinds.path_parents.contains(tree.parsed.kind(parent)) || self.names_item(node) {
            return false;
        }
        if tree.is(parent, MACRO_CALL) {
            return tree.field(parent, Field::Macro) != Some(node);
        }
        !(place.in_macro && tree.is(parent, TOKENS) && self.path_token(node))
    }

    // A token after `.` or `::` is a member name or a path tail, and a token before `::` or `!`
    // is a path head or a macro name.
    fn path_token(&self, node: NodeId) -> bool {
        let after = self.tree.parsed.next_sibling(node);
        matches!(self.token_before(node), Some("." | "::"))
            || matches!(after.map(|token| self.tree.text(token)), Some("::" | "!"))
    }

    fn token_before(&self, node: NodeId) -> Option<&'a str> {
        let before = self.tree.parsed.prev_sibling(node)?;
        Some(self.tree.text(before))
    }

    // A binding of the same name and owner whose names are visible where the use starts.
    fn visible_local(&self, node: NodeId, place: Place) -> bool {
        let start = self.tree.parsed.byte_range(node).start;
        let visible = |local: &Local| local.owner == place.owner && local.visible.contains(&start);
        self.locals
            .get(self.tree.text(node))
            .is_some_and(|locals| locals.iter().any(visible))
    }
}

fn places(tree: &Tree, kinds: &ScopeKinds) -> Vec<Place> {
    let parsed = tree.parsed;
    let mut places = vec![Place::default(); parsed.node_count() as usize];
    for node in tree.preorder(tree.root()) {
        let Some(parent) = parsed.parent(node) else {
            continue;
        };
        let above = places[parent.index()];
        let kind = parsed.kind(parent);
        places[node.index()] = Place {
            owner: kinds
                .owners
                .contains(kind)
                .then_some(parent)
                .or(above.owner),
            opaque: above.opaque || kinds.opaque.contains(kind),
            in_macro: above.in_macro || (tree.is(parent, MACRO_CALL) && tree.is(node, TOKENS)),
        };
    }
    places
}

// Each pattern of a binding site, with the bytes where its names are visible.
fn site_patterns(tree: &Tree, node: NodeId, site: Site) -> Vec<(NodeId, Range<usize>)> {
    match site {
        Site::Parameters => parameters(tree, node, tree.field(node, Field::Body)),
        Site::Signature => parameters(tree, node, None),
        _ => single_pattern(tree, node, site).into_iter().collect(),
    }
}

// Each parameter of the node's list, a pattern or a closure's bare pattern, visible in `body`.
fn parameters(tree: &Tree, node: NodeId, body: Option<NodeId>) -> Vec<(NodeId, Range<usize>)> {
    let Some(list) = tree.field(node, Field::Parameters) else {
        return Vec::new();
    };
    let visible = body.map_or(0..0, |body| tree.parsed.byte_range(body));
    let each = tree.named_children(list);
    each.map(|parameter| (parameter, visible.clone())).collect()
}

fn single_pattern(tree: &Tree, node: NodeId, site: Site) -> Option<(NodeId, Range<usize>)> {
    let parsed = tree.parsed;
    let end = parsed.byte_range(node).end;
    let pattern = tree.field(node, Field::Pattern);
    match site {
        Site::Parameters | Site::Signature => None,
        Site::Let => pattern
            .zip(parsed.parent(node))
            .map(|(pattern, block)| (pattern, end..parsed.byte_range(block).end)),
        Site::Condition => pattern.map(|pattern| (pattern, end..condition_end(tree, node))),
        Site::Arm => {
            arm_pattern(tree, node).map(|pattern| (pattern, parsed.byte_range(pattern).end..end))
        }
        Site::For => pattern
            .zip(tree.field(node, Field::Body))
            .map(|(pattern, body)| (pattern, parsed.byte_range(body))),
    }
}

fn arm_pattern(tree: &Tree, arm: NodeId) -> Option<NodeId> {
    let matched = tree.field(arm, Field::Pattern)?;
    let guard = tree.field(matched, Field::Condition);
    tree.named_children(matched)
        .find(|part| Some(*part) != guard)
}

// The end of the block that an `if` guards, else the end of the `while` or the match arm that
// holds the condition; never the `else`.
fn condition_end(tree: &Tree, condition: NodeId) -> usize {
    let parsed = tree.parsed;
    let mut node = condition;
    while let Some(parent) = parsed.parent(node) {
        if !tree.is_any(parent, &CONDITION_WRAPPERS) {
            let guarded = tree
                .is(parent, IF)
                .then(|| tree.field(parent, Field::Consequence));
            return parsed.byte_range(guarded.flatten().unwrap_or(parent)).end;
        }
        node = parent;
    }
    parsed.byte_range(condition).end
}

// The nodes that name what a pattern binds: each identifier in binding position and each
// shorthand field name. An identifier that starts upper case is a constant or a unit variant.
fn pattern_names(tree: &Tree, kinds: &ScopeKinds, pattern: NodeId) -> Vec<NodeId> {
    let mut names = Vec::new();
    let mut pending = vec![pattern];
    while let Some(node) = pending.pop() {
        if tree.is(node, IDENTIFIER) {
            if !upper_case_start(tree.text(node)) {
                names.push(node);
            }
        } else if tree.is(node, SHORTHAND) {
            names.push(node);
        } else {
            pending.extend(sub_patterns(tree, kinds, node));
        }
    }
    names
}

fn sub_patterns(tree: &Tree, kinds: &ScopeKinds, node: NodeId) -> Vec<NodeId> {
    let kind = tree.parsed.kind(node);
    if kinds.nested_patterns.contains(kind) {
        return tree.named_children(node).collect();
    }
    if kinds.typed_patterns.contains(kind) {
        let path = tree.field(node, Field::Type);
        return tree
            .named_children(node)
            .filter(|part| Some(*part) != path)
            .collect();
    }
    if !tree.is(node, FIELD_PATTERN) {
        return Vec::new();
    }
    match tree.field(node, Field::Pattern) {
        Some(inner) => vec![inner],
        None => tree.named_children(node).collect(),
    }
}

// `T::f`, `Self::f`, `<T as Tr>::f`, `Vec::<T>::new` and `Enum::Variant`: the last segment of a
// path whose qualifier is a type.
fn type_path_tail(tree: &Tree, node: NodeId) -> bool {
    let Some(scoped) = tree.parsed.parent(node) else {
        return false;
    };
    if !tree.is_any(scoped, &RUST_PATHS) || tree.field(scoped, Field::Name) != Some(node) {
        return false;
    }
    let Some(path) = tree.field(scoped, Field::Path) else {
        return false;
    };
    let last = tree.text(path).rsplit("::").next().unwrap_or_default();
    tree.is_any(path, &TYPE_QUALIFIERS)
        || (tree.is_any(path, &NAMED_QUALIFIERS) && upper_case_start(last.trim()))
}

fn upper_case_start(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use syntax::Language;

    use super::{
        CONDITION_WRAPPERS, FIELD_PATTERN, IDENTIFIER, IF, MACRO_CALL, MEMBER, NAMED_QUALIFIERS,
        RUST_PATHS, SHORTHAND, SITES, TOKENS, TYPE_QUALIFIERS, VARIANT,
    };

    #[test]
    fn every_kind_that_the_walk_names_exists_in_the_rust_grammar() {
        let sites = SITES.map(|(kind, _)| kind);
        let single = [
            IDENTIFIER,
            MEMBER,
            SHORTHAND,
            FIELD_PATTERN,
            VARIANT,
            MACRO_CALL,
            TOKENS,
            IF,
        ];
        let lists: [&[&str]; 6] = [
            &sites,
            &single,
            &RUST_PATHS,
            &CONDITION_WRAPPERS,
            &TYPE_QUALIFIERS,
            &NAMED_QUALIFIERS,
        ];
        for kind in lists.concat() {
            assert!(!Language::Rust.kinds(&[kind]).is_empty(), "{kind}");
        }
    }
}
