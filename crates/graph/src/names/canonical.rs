//! The owned form of a name index, in list order, that an update is compared by.

use std::collections::HashMap;
use std::ptr;

use serde_json::{Value, json};

use super::NameIndex;

impl<'a> NameIndex<'a> {
    // Each name that a file spells, in name order.
    pub(crate) fn names(&self) -> Vec<&'a str> {
        let mut names: Vec<&'a str> = self.positions.keys().copied().collect();
        names.sort_unstable();
        names
    }

    // Each name that a file spells, in name order, with its lists, counts and candidate order. A definition or a
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
                        assert!(ptr::eq(definition.file(), self.files[file]));
                        assert!(ptr::eq(
                            definition.symbol(),
                            &raw const self.files[file].facts.symbols[symbol]
                        ));
                        json!([definition.path(), symbol])
                    })
                    .collect();
                let sites: Vec<Value> = (entry.sites.iter())
                    .map(|site| {
                        assert!(ptr::eq(site.file, self.files[at[site.path()]]));
                        let reference = site.reference;
                        let role = reference.role.as_str();
                        json!([site.path(), reference.line, role, reference.qualifier])
                    })
                    .collect();
                let ordered: Vec<Value> = (entry.candidates.ordered.iter())
                    .map(|definition| json!([definition.path(), definition.position().1]))
                    .collect();
                json!([
                    name,
                    definitions,
                    sites,
                    entry.site_places,
                    entry.defining,
                    entry.referencing,
                    entry.spelling,
                    ordered,
                    entry.candidates.groups
                ])
            })
            .collect();
        Value::from(rows)
    }
}
