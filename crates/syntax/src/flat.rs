use tree_sitter::{Node, Tree, TreeCursor};

use crate::kinds::KindTable;

pub(crate) const NAMED: u8 = 1;
pub(crate) const MISSING: u8 = 2;
pub(crate) const ERROR: u8 = 4;
pub(crate) const HAS_ERROR: u8 = 8;
pub(crate) const NO_PARENT: u32 = u32::MAX;

// One node in cursor preorder. Its children start at the next index, and each sibling starts at the
// previous sibling's subtree_end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) parent: u32,
    pub(crate) subtree_end: u32,
    pub(crate) kind: u16,
    pub(crate) field: u16,
    pub(crate) flags: u8,
}

const _: () = assert!(size_of::<Record>() == 24);

pub(crate) struct Flat {
    pub(crate) records: Vec<Record>,
    line_starts: Vec<u32>,
}

// Why a tree cannot be flattened: a count above u32, or the index of a node off a char boundary.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FlatError {
    TooLarge,
    Boundary(u32),
}

impl Flat {
    pub(crate) fn build(tree: &Tree, source: &str, kinds: &KindTable) -> Result<Flat, FlatError> {
        let records = records(tree, kinds)?;
        if let Some(index) = records
            .iter()
            .position(|record| !on_boundaries(record, source))
        {
            return Err(FlatError::Boundary(index_u32(index)?));
        }
        Ok(Flat {
            records,
            line_starts: line_starts(source)?,
        })
    }

    pub(crate) fn record(&self, index: u32) -> &Record {
        &self.records[index as usize]
    }

    // Rows count line feeds only, and columns count bytes, as tree-sitter's points do.
    pub(crate) fn point(&self, byte: u32) -> (u32, u32) {
        let row = self.line_starts.partition_point(|start| *start <= byte) - 1;
        (
            index_u32(row).unwrap_or(u32::MAX),
            byte - self.line_starts[row],
        )
    }

    pub(crate) fn children(&self, index: u32) -> impl Iterator<Item = u32> + '_ {
        let end = self.record(index).subtree_end;
        let mut next = index + 1;
        core::iter::from_fn(move || {
            (next < end).then(|| {
                let child = next;
                next = self.record(child).subtree_end;
                child
            })
        })
    }
}

fn records(tree: &Tree, kinds: &KindTable) -> Result<Vec<Record>, FlatError> {
    let mut records: Vec<Record> = Vec::new();
    let mut open: Vec<u32> = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let index = index_u32(records.len())?;
        records.push(record(&cursor, &open, kinds)?);
        if cursor.goto_first_child() {
            open.push(index);
            continue;
        }
        records[index as usize].subtree_end = index + 1;
        if !climb(&mut cursor, &mut open, &mut records)? {
            return Ok(records);
        }
    }
}

// Move to the next sibling of the current node or of its nearest ancestor, closing each subtree
// left behind. Return false at the end of the walk.
fn climb(
    cursor: &mut TreeCursor,
    open: &mut Vec<u32>,
    records: &mut [Record],
) -> Result<bool, FlatError> {
    while !cursor.goto_next_sibling() {
        if !cursor.goto_parent() {
            return Ok(false);
        }
        let closed = open
            .pop()
            .expect("the open stack holds one entry per cursor level");
        records[closed as usize].subtree_end = index_u32(records.len())?;
    }
    Ok(true)
}

fn record(cursor: &TreeCursor, open: &[u32], kinds: &KindTable) -> Result<Record, FlatError> {
    let node = cursor.node();
    Ok(Record {
        start: index_u32(node.start_byte())?,
        end: index_u32(node.end_byte())?,
        parent: open.last().copied().unwrap_or(NO_PARENT),
        subtree_end: 0,
        kind: kinds
            .id(node.kind())
            .expect("tree-sitter names each node from the grammar's name table"),
        field: cursor.field_id().map_or(0, core::num::NonZeroU16::get),
        flags: flags(node),
    })
}

fn flags(node: Node) -> u8 {
    let mut flags = 0;
    for (set, bit) in [
        (node.is_named(), NAMED),
        (node.is_missing(), MISSING),
        (node.is_error(), ERROR),
        (node.has_error(), HAS_ERROR),
    ] {
        if set {
            flags |= bit;
        }
    }
    flags
}

fn on_boundaries(record: &Record, source: &str) -> bool {
    source.is_char_boundary(record.start as usize) && source.is_char_boundary(record.end as usize)
}

fn line_starts(source: &str) -> Result<Vec<u32>, FlatError> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index_u32(index + 1)?);
        }
    }
    Ok(starts)
}

fn index_u32(index: usize) -> Result<u32, FlatError> {
    u32::try_from(index).map_err(|_| FlatError::TooLarge)
}
