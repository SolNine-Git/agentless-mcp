use std::sync::OnceLock;

use crate::language::Language;

// tree-sitter's names for its two built-in symbols, which no grammar lists in its symbol count.
const BUILT_IN: [u16; 2] = [u16::MAX, u16::MAX - 1];

// The distinct kind names of one grammar. A node's id is the index of the name that tree-sitter
// gives it: scala's public symbol of "(" names "arguments", so a public kind id loses the name.
pub(crate) struct KindTable {
    names: Vec<&'static str>,
    by_address: Vec<(usize, u16)>,
}

static TABLES: [OnceLock<KindTable>; 22] = [const { OnceLock::new() }; 22];

pub(crate) fn table(language: Language) -> &'static KindTable {
    TABLES[language as usize].get_or_init(|| KindTable::build(&language.grammar()))
}

impl KindTable {
    fn build(grammar: &tree_sitter::Language) -> KindTable {
        let count = u16::try_from(grammar.node_kind_count()).unwrap_or(u16::MAX - 2);
        let symbols: Vec<&'static str> = (0..count)
            .chain(BUILT_IN)
            .filter_map(|symbol| grammar.node_kind_for_id(symbol))
            .collect();
        let mut names = symbols.clone();
        names.sort_unstable();
        names.dedup();
        let mut by_address: Vec<(usize, u16)> = symbols
            .iter()
            .filter_map(|name| Some((name.as_ptr().addr(), index_of(&names, name)?)))
            .collect();
        by_address.sort_unstable();
        by_address.dedup();
        KindTable { names, by_address }
    }

    // tree-sitter returns each name from the grammar's own name table, so its address finds it.
    pub(crate) fn id(&self, name: &'static str) -> Option<u16> {
        let address = name.as_ptr().addr();
        self.by_address
            .binary_search_by_key(&address, |(address, _)| *address)
            .ok()
            .map(|found| self.by_address[found].1)
            .or_else(|| index_of(&self.names, name))
    }

    pub(crate) fn id_by_name(&self, name: &str) -> Option<u16> {
        index_of(&self.names, name)
    }

    pub(crate) fn name(&self, id: u16) -> &'static str {
        self.names[usize::from(id)]
    }

    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }
}

fn index_of(names: &[&'static str], name: &str) -> Option<u16> {
    let found = names.binary_search(&name).ok()?;
    u16::try_from(found).ok()
}
