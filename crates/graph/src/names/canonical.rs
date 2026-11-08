//! The owned form of a name index, in list order, that an update is compared by.

use std::collections::HashMap;
use std::ptr;

use serde_json::{Value, json};

use super::NameIndex;

impl NameIndex<'_> {
    // Each name that a file spells, in name order, with its lists and counts. A definition or a
    // site must point at the facts of its file in the index's own scan.
    pub(crate) fn canonical(&self) -> Value {
        let at: HashMap<&str, usize> = (self.files.iter().enumerate())
            .map(|(position, file)| (file.path.as_str(), position))
            .collect();
        let mut names: Vec<(&str, usize)> = (self.positions.iter())
            .map(|(name, position)| (*name, *position))
            .collect();
        names.sort_unstable();
        let rows: Vec<Value> = (names.iter())
            .map(|&(name, position)| {
                let entry = &self.entries[position];
                let definitions: Vec<Value> = (entry.definitions.iter())
                    .map(|definition| {
                        let (file, symbol) = definition.position();
                        assert!(ptr::eq(definition.file(), &raw const self.files[file]));
                        assert!(ptr::eq(
                            definition.symbol(),
                            &raw const self.files[file].facts.symbols[symbol]
                        ));
                        json!([definition.path(), symbol])
                    })
                    .collect();
                let sites: Vec<Value> = (entry.sites.iter())
                    .map(|site| {
                        assert!(ptr::eq(site.file, &raw const self.files[at[site.path()]]));
                        let reference = site.reference;
                        let role = reference.role.as_str();
                        json!([site.path(), reference.line, role, reference.qualifier])
                    })
                    .collect();
                json!([
                    name,
                    definitions,
                    sites,
                    entry.defining,
                    entry.referencing,
                    entry.spelling
                ])
            })
            .collect();
        Value::from(rows)
    }
}
