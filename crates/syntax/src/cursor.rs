//! A cursor for many child-by-field lookups over one parse.

use tree_sitter::TreeCursor;

use crate::language::FieldId;
use crate::parsed::{NodeId, Parsed, descendant_offset};

/// Answers child-by-field lookups as [`Parsed::child_by_field`] does, moving from the last node.
pub struct FieldCursor<'p> {
    parsed: &'p Parsed,
    // At node `at`. It makes only the moves that built the flat table, so the two stay in step.
    cursor: TreeCursor<'p>,
    at: u32,
    probe: TreeCursor<'p>,
}

impl<'p> FieldCursor<'p> {
    /// Return a cursor at the root of `parsed`.
    #[must_use]
    pub fn new(parsed: &'p Parsed) -> FieldCursor<'p> {
        FieldCursor {
            parsed,
            cursor: parsed.tree.walk(),
            at: 0,
            probe: parsed.tree.walk(),
        }
    }

    /// Return the parse that the cursor walks.
    #[must_use]
    pub fn parsed(&self) -> &'p Parsed {
        self.parsed
    }

    /// Return what tree-sitter's child-by-field lookup returns, which can be a grandchild.
    pub fn child_by_field(&mut self, node: NodeId, field: FieldId) -> Option<NodeId> {
        self.goto(node.0);
        let from = self.cursor.node();
        let found = from.child_by_field_id(field.0.get())?;
        self.probe.reset(from);
        descendant_offset(&mut self.probe, found).map(|offset| NodeId(node.0 + offset))
    }

    fn goto(&mut self, target: u32) {
        self.climb(target);
        while self.at != target {
            moved(self.cursor.goto_first_child());
            self.at += 1;
            self.skip_to(target);
        }
    }

    // Up to the nearest node whose subtree holds `target`. A target after the current node in
    // its parent's subtree is reached across the siblings instead.
    fn climb(&mut self, target: u32) {
        while !self.holds(self.at, target) {
            let parent = self.parsed.flat.record(self.at).parent;
            if target > self.at && self.holds(parent, target) {
                self.skip_to(target);
                return;
            }
            moved(self.cursor.goto_parent());
            self.at = parent;
        }
    }

    // Along the siblings until the current node's subtree holds `target`.
    fn skip_to(&mut self, target: u32) {
        while !self.holds(self.at, target) {
            moved(self.cursor.goto_next_sibling());
            self.at = self.parsed.flat.record(self.at).subtree_end;
        }
    }

    fn holds(&self, ancestor: u32, node: u32) -> bool {
        ancestor <= node && node < self.parsed.flat.record(ancestor).subtree_end
    }
}

fn moved(done: bool) {
    assert!(done, "each move matches the flat walk of the parse");
}
