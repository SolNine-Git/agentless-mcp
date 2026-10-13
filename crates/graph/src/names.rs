//! The name index: where each name is defined and where it is spelled, across one scan.

use std::collections::BTreeSet;

use facts::{Ref, StableId, Symbol};

use crate::scan::{FileFacts, in_name_graph};

const NOISE_NAME_LENGTH: usize = 2;
const NOISE_NAME_MULTIPLIER: f64 = 0.1;

/// A symbol together with the file that declares it.
#[derive(Clone, Copy, Debug)]
pub struct Definition<'a> {
    file: &'a FileFacts,
    symbol: &'a Symbol,
    file_index: usize,
    symbol_index: usize,
}

impl<'a> Definition<'a> {
    pub(crate) fn new(files: &'a [FileFacts], file_index: usize, symbol_index: usize) -> Self {
        let file = &files[file_index];
        Definition {
            file,
            symbol: &file.facts.symbols[symbol_index],
            file_index,
            symbol_index,
        }
    }

    /// Return the declaring file.
    #[must_use]
    pub fn file(&self) -> &'a FileFacts {
        self.file
    }

    /// Return the declaring file's repository-relative path.
    #[must_use]
    pub fn path(&self) -> &'a str {
        &self.file.path
    }

    /// Return the symbol.
    #[must_use]
    pub fn symbol(&self) -> &'a Symbol {
        self.symbol
    }

    /// Return the symbol's stable id.
    #[must_use]
    pub fn stable_id(&self) -> StableId {
        StableId::of(self.file.language, &self.file.path, self.symbol)
    }

    // The position of the file in the scan and of the symbol in the file.
    pub(crate) fn position(&self) -> (usize, usize) {
        (self.file_index, self.symbol_index)
    }
}

/// One spelling of a name: the file and the identifier occurrence.
#[derive(Clone, Copy, Debug)]
pub struct Site<'a> {
    pub file: &'a FileFacts,
    pub reference: &'a Ref,
}

impl<'a> Site<'a> {
    /// Return the spelling file's repository-relative path.
    #[must_use]
    pub fn path(&self) -> &'a str {
        &self.file.path
    }
}

#[derive(Default)]
struct NameEntry<'a> {
    definitions: Vec<Definition<'a>>,
    sites: Vec<Site<'a>>,
    defining: Vec<&'a str>,
    referencing: u32,
    spelling: u32,
    last_referencing: Option<usize>,
    last_spelling: Option<usize>,
}

/// Name-keyed definitions and spellings across the name-graph files of one scan.
pub struct NameIndex<'a> {
    files: &'a [FileFacts],
    names: Vec<&'a str>,
    entries: Vec<NameEntry<'a>>,
}

impl<'a> NameIndex<'a> {
    /// Index `files` by name, each list in scan order; data files stay out.
    #[must_use]
    pub fn build(files: &'a [FileFacts]) -> NameIndex<'a> {
        let names = sorted_names(files);
        let mut index = NameIndex {
            files,
            entries: names.iter().map(|_| NameEntry::default()).collect(),
            names,
        };
        for (file_index, file) in files.iter().enumerate() {
            if in_name_graph(file) {
                index.add_file(file_index, file);
            }
        }
        for entry in &mut index.entries {
            entry.defining = entry.definitions.iter().map(Definition::path).collect();
            entry.defining.sort_unstable();
            entry.defining.dedup();
        }
        index
    }

    /// Return the scanned files the index was built from.
    #[must_use]
    pub fn files(&self) -> &'a [FileFacts] {
        self.files
    }

    /// Return the definitions of `name`, in scan order.
    #[must_use]
    pub fn definitions(&self, name: &str) -> &[Definition<'a>] {
        self.listed(name, |entry| &entry.definitions)
    }

    /// Return every spelling of `name`, whatever its role, in scan order.
    #[must_use]
    pub fn sites(&self, name: &str) -> &[Site<'a>] {
        self.listed(name, |entry| &entry.sites)
    }

    /// Return the distinct files that define `name`, in path order.
    #[must_use]
    pub fn defining_paths(&self, name: &str) -> &[&'a str] {
        self.listed(name, |entry| &entry.defining)
    }

    /// Return how many files reference `name`, or 1 when none does (the shared-caller spread).
    #[must_use]
    pub fn caller_spread(&self, name: &str) -> u32 {
        match self.counted(name, |entry| entry.referencing) {
            0 => 1,
            files => files,
        }
    }

    /// Return how many files spell `name` in any role, 0 when none does (the map's spread).
    #[must_use]
    pub fn spelling_spread(&self, name: &str) -> u32 {
        self.counted(name, |entry| entry.spelling)
    }

    // The edge-weight spread: every name a reference counts is a key, so it is at least 1.
    pub(crate) fn edge_spread(&self, name: &str) -> u32 {
        self.counted(name, |entry| entry.referencing)
    }

    pub(crate) fn position(&self, name: &str) -> Option<usize> {
        self.names.binary_search(&name).ok()
    }

    pub(crate) fn definitions_at(&self, position: usize) -> &[Definition<'a>] {
        &self.entries[position].definitions
    }

    pub(crate) fn name_count(&self) -> usize {
        self.names.len()
    }

    fn listed<'s, T>(&'s self, name: &str, pick: impl Fn(&'s NameEntry<'a>) -> &'s [T]) -> &'s [T] {
        self.position(name)
            .map_or(&[], |found| pick(&self.entries[found]))
    }

    fn counted(&self, name: &str, pick: impl Fn(&NameEntry<'a>) -> u32) -> u32 {
        self.position(name)
            .map_or(0, |found| pick(&self.entries[found]))
    }

    fn entry_mut(&mut self, name: &str) -> &mut NameEntry<'a> {
        let found = self
            .position(name)
            .expect("the index holds every name its own files spell");
        &mut self.entries[found]
    }

    fn add_file(&mut self, file_index: usize, file: &'a FileFacts) {
        for (symbol_index, symbol) in file.facts.symbols.iter().enumerate() {
            let definition = Definition::new(self.files, file_index, symbol_index);
            self.entry_mut(&symbol.name).definitions.push(definition);
        }
        for reference in &file.facts.refs {
            let entry = self.entry_mut(&reference.name);
            entry.sites.push(Site { file, reference });
            count_file(&mut entry.spelling, &mut entry.last_spelling, file_index);
            if reference.is_reference() {
                count_file(
                    &mut entry.referencing,
                    &mut entry.last_referencing,
                    file_index,
                );
            }
        }
    }
}

// A file's spellings arrive together, so a new file index is a new distinct file.
fn count_file(count: &mut u32, last: &mut Option<usize>, file_index: usize) {
    if *last != Some(file_index) {
        *count += 1;
        *last = Some(file_index);
    }
}

fn sorted_names(files: &[FileFacts]) -> Vec<&str> {
    let mut names: Vec<&str> = files
        .iter()
        .filter(|file| in_name_graph(file))
        .flat_map(|file| {
            let symbols = file.facts.symbols.iter().map(|symbol| symbol.name.as_str());
            symbols.chain(
                file.facts
                    .refs
                    .iter()
                    .map(|reference| reference.name.as_str()),
            )
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Return the weight multiplier of `name`: a name of two code points or fewer, or a listed one,
/// counts a tenth.
#[must_use]
pub fn name_multiplier(name: &str, stoplist: &BTreeSet<String>) -> f64 {
    if name.chars().count() <= NOISE_NAME_LENGTH || stoplist.contains(name) {
        NOISE_NAME_MULTIPLIER
    } else {
        1.0
    }
}

/// Return the divisor for a name spelled in `spread` files: `1 + ln(1 + spread)`.
#[must_use]
pub fn common_name_damping(spread: u32) -> f64 {
    1.0 + (1.0 + f64::from(spread)).ln()
}
