use std::sync::Arc;

use tree_sitter::{Node, Parser, Tree, TreeCursor};

use crate::flat::{ERROR, Flat, FlatError, HAS_ERROR, MISSING, NAMED, NO_PARENT};
use crate::kinds;
use crate::language::{FieldId, KindId, KindSet, Language};

/// A node of one parse, by its index in document order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    /// Return the node's position in document order, 0 for the root.
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A position in the source: a zero-based row, and a column in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub row: u32,
    pub column: u32,
}

/// Why a source did not yield a tree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unparsable {
    #[error("the {language:?} grammar has ABI {abi}, outside the range of the runtime")]
    Abi { language: Language, abi: usize },
    #[error("the {language:?} parser returned no tree")]
    NoTree { language: Language },
    #[error("the source has {len} bytes or nodes, above the u32 offset range")]
    TooLarge { len: usize },
    #[error("node {node} of the {language:?} tree does not start and end on a character boundary")]
    Boundary { language: Language, node: u32 },
}

/// A parsed source: its tree, a flat table of its nodes, and the source text.
pub struct Parsed {
    language: Language,
    source: Arc<str>,
    pub(crate) tree: Tree,
    pub(crate) flat: Flat,
}

/// Parse all of `source` as `language`.
///
/// # Errors
/// Returns `Unparsable` when the grammar cannot load, the parser returns no tree, the source is
/// above the u32 offset range, or a node falls off a character boundary.
pub fn parse(language: Language, source: Arc<str>) -> Result<Parsed, Unparsable> {
    if u32::try_from(source.len()).is_err() {
        return Err(Unparsable::TooLarge { len: source.len() });
    }
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar())
        .map_err(|_| Unparsable::Abi {
            language,
            abi: language.abi_version(),
        })?;
    let tree = parser
        .parse(source.as_bytes(), None)
        .ok_or(Unparsable::NoTree { language })?;
    let flat =
        Flat::build(&tree, &source, kinds::table(language)).map_err(|error| match error {
            FlatError::TooLarge => Unparsable::TooLarge { len: source.len() },
            FlatError::Boundary(node) => Unparsable::Boundary { language, node },
        })?;
    Ok(Parsed {
        language,
        source,
        tree,
        flat,
    })
}

impl Parsed {
    /// Return the language the source was parsed as.
    #[must_use]
    pub fn language(&self) -> Language {
        self.language
    }

    /// Return the source text.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Return the root node.
    #[must_use]
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Return the number of nodes, named and anonymous.
    #[must_use]
    pub fn node_count(&self) -> u32 {
        u32::try_from(self.flat.records.len()).unwrap_or(u32::MAX)
    }

    /// Return the node's kind id.
    #[must_use]
    pub fn kind(&self, node: NodeId) -> KindId {
        KindId(self.flat.record(node.0).kind)
    }

    /// Return the node's kind name, `ERROR` for an error node.
    #[must_use]
    pub fn kind_name(&self, node: NodeId) -> &'static str {
        kinds::table(self.language).name(self.flat.record(node.0).kind)
    }

    /// Return whether the node is named.
    #[must_use]
    pub fn is_named(&self, node: NodeId) -> bool {
        self.flag(node, NAMED)
    }

    /// Return whether the parser inserted the node to recover from an error.
    #[must_use]
    pub fn is_missing(&self, node: NodeId) -> bool {
        self.flag(node, MISSING)
    }

    /// Return whether the node is an ERROR node.
    #[must_use]
    pub fn is_error(&self, node: NodeId) -> bool {
        self.flag(node, ERROR)
    }

    /// Return whether the node's subtree holds an ERROR or MISSING node.
    #[must_use]
    pub fn has_error(&self, node: NodeId) -> bool {
        self.flag(node, HAS_ERROR)
    }

    /// Return the node's first byte offset.
    #[must_use]
    pub fn start_byte(&self, node: NodeId) -> u32 {
        self.flat.record(node.0).start
    }

    /// Return the byte offset just past the node.
    #[must_use]
    pub fn end_byte(&self, node: NodeId) -> u32 {
        self.flat.record(node.0).end
    }

    /// Return the node's byte range.
    #[must_use]
    pub fn byte_range(&self, node: NodeId) -> core::ops::Range<usize> {
        let record = self.flat.record(node.0);
        record.start as usize..record.end as usize
    }

    /// Return the node's text.
    #[must_use]
    pub fn text(&self, node: NodeId) -> &str {
        &self.source[self.byte_range(node)]
    }

    /// Return the node's start point.
    #[must_use]
    pub fn start_point(&self, node: NodeId) -> Point {
        point(self.flat.point(self.start_byte(node)))
    }

    /// Return the node's end point.
    #[must_use]
    pub fn end_point(&self, node: NodeId) -> Point {
        point(self.flat.point(self.end_byte(node)))
    }

    /// Return the node's parent, or None for the root.
    #[must_use]
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        let parent = self.flat.record(node.0).parent;
        (parent != NO_PARENT).then_some(NodeId(parent))
    }

    /// Return the sibling right before the node, or None for a first child or the root.
    #[must_use]
    pub fn prev_sibling(&self, node: NodeId) -> Option<NodeId> {
        let parent = self.parent(node)?;
        // In preorder the record before a node is its parent or the last node of the subtree of
        // its previous sibling, so the walk up from there costs the depth, not the sibling count.
        let mut current = NodeId(node.0.checked_sub(1)?);
        loop {
            if current == parent {
                return None;
            }
            let up = self.parent(current)?;
            if up == parent {
                return Some(current);
            }
            current = up;
        }
    }

    /// Return the node's children, named and anonymous, in order.
    pub fn children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.flat.children(node.0).map(NodeId)
    }

    /// Return the node's named children in order.
    pub fn named_children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(node).filter(|child| self.is_named(*child))
    }

    /// Return the number of the node's children.
    #[must_use]
    pub fn child_count(&self, node: NodeId) -> usize {
        self.children(node).count()
    }

    /// Return the number of the node's named children.
    #[must_use]
    pub fn named_child_count(&self, node: NodeId) -> usize {
        self.named_children(node).count()
    }

    /// Return whether the node has no children.
    #[must_use]
    pub fn is_leaf(&self, node: NodeId) -> bool {
        self.flat.record(node.0).subtree_end == node.0 + 1
    }

    /// Return the node and every node below it, in document order.
    pub fn descendants(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        (node.0..self.flat.record(node.0).subtree_end).map(NodeId)
    }

    /// Return the field under which the node hangs from its parent.
    #[must_use]
    pub fn field_of(&self, node: NodeId) -> Option<FieldId> {
        core::num::NonZeroU16::new(self.flat.record(node.0).field).map(FieldId)
    }

    /// Return what tree-sitter's child-by-field lookup returns, which can be a grandchild. Each
    /// call descends from the root at O(depth) cost, so a walk reads fields through `FieldCursor`.
    #[must_use]
    pub fn child_by_field(&self, node: NodeId, field: FieldId) -> Option<NodeId> {
        let mut cursor = self.tree.walk();
        cursor.goto_descendant(node.0 as usize);
        let from = cursor.node();
        let found = from.child_by_field_id(field.0.get())?;
        descendant_offset(&mut from.walk(), found).map(|offset| NodeId(node.0 + offset))
    }

    /// Return the node's children that hang under `field`, in order.
    pub fn children_by_field(
        &self,
        node: NodeId,
        field: FieldId,
    ) -> impl Iterator<Item = NodeId> + '_ {
        self.children(node)
            .filter(move |child| self.field_of(*child) == Some(field))
    }

    /// Return the first node of a kind in `kinds`, in document order, from `node` itself down.
    #[must_use]
    pub fn first_of_kind(&self, node: NodeId, kinds: &KindSet) -> Option<NodeId> {
        self.descendants(node)
            .find(|candidate| kinds.contains(self.kind(*candidate)))
    }

    /// Return the first ERROR or MISSING node in document order.
    #[must_use]
    pub fn first_error(&self) -> Option<NodeId> {
        self.descendants(self.root())
            .find(|node| self.is_error(*node) || self.is_missing(*node))
    }

    /// Return the first node in document order whose subtree holds an error and none of whose
    /// children's subtrees does, or `None` for a tree with no error.
    #[must_use]
    pub fn innermost_error(&self) -> Option<NodeId> {
        let mut node = self.root();
        if !self.has_error(node) {
            return None;
        }
        while let Some(child) = self.children(node).find(|child| self.has_error(*child)) {
            node = child;
        }
        Some(node)
    }

    pub(crate) fn node_id(&self, node: Node) -> Option<NodeId> {
        descendant_offset(&mut self.tree.walk(), node).map(NodeId)
    }

    fn flag(&self, node: NodeId, bit: u8) -> bool {
        self.flat.record(node.0).flags & bit != 0
    }
}

fn point((row, column): (u32, u32)) -> Point {
    Point { row, column }
}

// The preorder offset of `target` below the node that `cursor` starts at, which equals their
// distance in the flat table.
pub(crate) fn descendant_offset(cursor: &mut TreeCursor, target: Node) -> Option<u32> {
    while cursor.node().id() != target.id() {
        let next = cursor.node().child_with_descendant(target)?;
        if !(cursor.goto_first_child() && goto_sibling(cursor, next)) {
            return None;
        }
    }
    u32::try_from(cursor.descendant_index()).ok()
}

fn goto_sibling(cursor: &mut TreeCursor, target: Node) -> bool {
    while cursor.node().id() != target.id() {
        if !cursor.goto_next_sibling() {
            return false;
        }
    }
    true
}
