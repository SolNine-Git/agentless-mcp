use std::cell::RefCell;
use std::sync::OnceLock;

use kernel::Span;
use syntax::{FieldCursor, FieldId, KindSet, Language, NodeId, Parsed};

// The fields that the walkers read by name.
#[derive(Clone, Copy)]
pub(super) enum Field {
    Alias,
    Arguments,
    Argument,
    Attribute,
    Body,
    DeclarationKind,
    Declarator,
    Function,
    Key,
    Left,
    List,
    ModuleName,
    Name,
    Object,
    Parameters,
    Path,
    Result,
    ReturnType,
    Right,
    Superclasses,
    Type,
    Value,
    Values,
    Variables,
}

const FIELD_NAMES: [&str; 24] = [
    "alias",
    "arguments",
    "argument",
    "attribute",
    "body",
    "declaration_kind",
    "declarator",
    "function",
    "key",
    "left",
    "list",
    "module_name",
    "name",
    "object",
    "parameters",
    "path",
    "result",
    "return_type",
    "right",
    "superclasses",
    "type",
    "value",
    "values",
    "variables",
];

type Fields = [Option<FieldId>; FIELD_NAMES.len()];

static FIELDS: [OnceLock<Fields>; Language::ALL.len()] = [const { OnceLock::new() }; 22];

// One parse, with the field ids of its grammar, and the node reads that the walkers share.
pub(super) struct Tree<'a> {
    pub(super) parsed: &'a Parsed,
    fields: &'static Fields,
    // The walkers share the tree by reference, and each field read moves the cursor.
    cursor: RefCell<FieldCursor<'a>>,
}

impl<'a> Tree<'a> {
    pub(super) fn new(parsed: &'a Parsed) -> Tree<'a> {
        let language = parsed.language();
        let fields =
            FIELDS[language as usize].get_or_init(|| FIELD_NAMES.map(|name| language.field(name)));
        Tree {
            parsed,
            fields,
            cursor: RefCell::new(FieldCursor::new(parsed)),
        }
    }

    pub(super) fn root(&self) -> NodeId {
        self.parsed.root()
    }

    pub(super) fn field(&self, node: NodeId, field: Field) -> Option<NodeId> {
        let field = self.field_id(field)?;
        self.cursor.borrow_mut().child_by_field(node, field)
    }

    pub(super) fn field_id(&self, field: Field) -> Option<FieldId> {
        self.fields[field as usize]
    }

    pub(super) fn text(&self, node: NodeId) -> &'a str {
        self.parsed.text(node)
    }

    pub(super) fn kind(&self, node: NodeId) -> &'static str {
        self.parsed.kind_name(node)
    }

    pub(super) fn is(&self, node: NodeId, kind: &str) -> bool {
        self.parsed.kind_name(node) == kind
    }

    pub(super) fn is_any(&self, node: NodeId, kinds: &[&str]) -> bool {
        kinds.contains(&self.parsed.kind_name(node))
    }

    // The 1-based line that the node starts on.
    pub(super) fn line(&self, node: NodeId) -> u32 {
        self.parsed.start_point(node).row.saturating_add(1)
    }

    // The 1-based line that the node ends on.
    pub(super) fn end_line(&self, node: NodeId) -> u32 {
        self.parsed.end_point(node).row.saturating_add(1)
    }

    pub(super) fn children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + 'a {
        self.parsed.children(node)
    }

    pub(super) fn named_children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + 'a {
        self.parsed.named_children(node)
    }

    pub(super) fn preorder(&self, node: NodeId) -> impl Iterator<Item = NodeId> + 'a {
        self.parsed.descendants(node)
    }

    // What `read` finds at each node of `kind` in the whole tree, in document order.
    pub(super) fn each_of_kind<T>(&self, kind: &str, read: impl Fn(NodeId) -> Option<T>) -> Vec<T> {
        self.each_where(|node| self.is(node, kind), read)
    }

    // What `read` finds at each node that `wanted` accepts in the whole tree, in document order.
    pub(super) fn each_where<T>(
        &self,
        wanted: impl Fn(NodeId) -> bool,
        read: impl Fn(NodeId) -> Option<T>,
    ) -> Vec<T> {
        self.preorder(self.root())
            .filter(|node| wanted(*node))
            .filter_map(read)
            .collect()
    }

    // The first child, named or not, of a kind in `kinds`.
    pub(super) fn first_child_in(&self, node: NodeId, kinds: &KindSet) -> Option<NodeId> {
        self.children(node)
            .find(|child| kinds.contains(self.parsed.kind(*child)))
    }

    // The first child, named or not, whose kind is one of `kinds`.
    pub(super) fn first_child_of(&self, node: NodeId, kinds: &[&str]) -> Option<NodeId> {
        self.children(node).find(|child| self.is_any(*child, kinds))
    }

    // Each node below `root` that `stop` accepts, in document order, with no descent into one.
    pub(super) fn declarations_under(
        &self,
        root: NodeId,
        stop: impl Fn(NodeId) -> bool,
    ) -> Vec<NodeId> {
        let mut found = Vec::new();
        let mut frames = vec![self.children(root)];
        while let Some(children) = frames.last_mut() {
            match children.next() {
                None => {
                    frames.pop();
                }
                Some(node) if stop(node) => found.push(node),
                Some(node) => frames.push(self.children(node)),
            }
        }
        found
    }

    // What `read` gives for each declaration of a kind in `kinds` below the root, in order.
    pub(super) fn each_declaration<T>(
        &self,
        kinds: &[&str],
        read: impl Fn(NodeId) -> Vec<T>,
    ) -> Vec<T> {
        let declarations = self.declarations_under(self.root(), |node| self.is_any(node, kinds));
        declarations.into_iter().flat_map(read).collect()
    }

    // The outermost ERROR nodes in document order; a MISSING node is not one of them.
    pub(super) fn error_spans(&self) -> Vec<Span> {
        let parsed = self.parsed;
        let mut spans = Vec::new();
        let mut pending = vec![parsed.root()];
        while let Some(node) = pending.pop() {
            if parsed.is_error(node) {
                let range = parsed.byte_range(node);
                spans.push(span(parsed.start_byte(node), parsed.end_byte(node), &range));
            } else if parsed.has_error(node) && !parsed.is_missing(node) {
                let children: Vec<NodeId> = parsed.children(node).collect();
                pending.extend(children.into_iter().rev());
            }
        }
        spans
    }
}

fn span(start: u32, end: u32, range: &core::ops::Range<usize>) -> Span {
    Span::new(start, end).unwrap_or_else(|| panic!("node bytes {range:?} end before they start"))
}
