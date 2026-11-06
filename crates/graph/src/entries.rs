//! The names that each Python package entry binds when it loads, with the object that each holds.

use std::collections::{BTreeMap, BTreeSet};

use facts::{Import, Runs};

use crate::imports::{
    is_package_entry, module_object, resolve_import_target, resolve_imported_submodule,
};
use crate::paths::PathIndex;
use crate::scan::FileFacts;

// What a name holds: a module file of this repository, or `None` for any other object.
type Object<'a> = Option<&'a str>;

// For each package entry and name, the objects that the entry binds the name to at module level.
#[derive(Debug, Default)]
pub(crate) struct EntryNames<'a> {
    objects: BTreeMap<(&'a str, &'a str), BTreeSet<Object<'a>>>,
}

impl<'a> EntryNames<'a> {
    pub(crate) fn build(files: &'a [FileFacts], paths: &PathIndex<'a>) -> EntryNames<'a> {
        let mut names = EntryNames::default();
        for file in files.iter().filter(|file| is_package_entry(&file.path)) {
            names.add(file, paths);
        }
        names
    }

    // A symbol with no parent holds a definition; an import that runs at load holds what it
    // binds. A star import names no member here.
    fn add(&mut self, file: &'a FileFacts, paths: &PathIndex<'a>) {
        let entry = file.path.as_str();
        let defined = file.facts.symbols.iter();
        for symbol in defined.filter(|symbol| symbol.parent.is_empty()) {
            self.insert(entry, &symbol.name, None);
        }
        let loaded = file.facts.imports.iter();
        for import in loaded.filter(|import| import.runs == Runs::Load && !import.binds_all) {
            for (name, object) in bound_objects(entry, import, paths) {
                self.insert(entry, name, object);
            }
        }
    }

    fn insert(&mut self, entry: &'a str, name: &'a str, object: Object<'a>) {
        self.objects
            .entry((entry, name))
            .or_default()
            .insert(object);
    }

    // Whether `entry` binds one of `members`, each with the module of its name, to another object.
    pub(crate) fn rebinds(&self, entry: &str, members: &[(&str, Object<'_>)]) -> bool {
        members.iter().any(|&(member, module)| {
            self.objects
                .get(&(entry, member))
                .is_some_and(|objects| objects.iter().any(|object| *object != module))
        })
    }
}

// `from m import x as n` binds n to the submodule m.x when one exists, else to an attribute of m;
// `import m as n` binds n to the module m.
fn bound_objects<'a>(
    entry: &str,
    import: &'a Import,
    paths: &PathIndex<'a>,
) -> Vec<(&'a str, Object<'a>)> {
    let level = import.relative_level;
    if import.names.is_empty() {
        let (binding, referenced) = module_object(import);
        return vec![(
            binding,
            resolve_import_target(entry, referenced, level, paths),
        )];
    }
    import
        .bound_names()
        .map(|(member, local)| {
            let module = resolve_imported_submodule(entry, &import.module, level, member, paths);
            (local, module)
        })
        .collect()
}
