//! The name index: where each name is defined and where it is spelled, across one scan.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use facts::{Ref, StableId, Symbol};

use crate::candidates::Candidates;
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
    pub(crate) fn new(files: &[&'a FileFacts], file_index: usize, symbol_index: usize) -> Self {
        let file = files[file_index];
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
    // The scan position of each site's file, beside `sites`.
    site_files: Vec<usize>,
    defining: Vec<&'a str>,
    candidates: Candidates<'a>,
    referencing: u32,
    spelling: u32,
    last_referencing: Option<usize>,
    last_spelling: Option<usize>,
}

/// Name-keyed definitions and spellings across the name-graph files of one scan.
pub struct NameIndex<'a> {
    files: &'a [&'a FileFacts],
    // Lookup only: every order the index gives out comes from the lists of an entry. A name that
    // no file spells after an update leaves the map, and its entry stays empty.
    positions: HashMap<&'a str, usize>,
    entries: Vec<NameEntry<'a>>,
}

impl<'a> NameIndex<'a> {
    /// Index `files` by name, each list in scan order; data files stay out.
    #[must_use]
    pub fn build(files: &'a [&'a FileFacts]) -> NameIndex<'a> {
        let names = sorted_names(files);
        let positions = names
            .iter()
            .enumerate()
            .map(|(at, name)| (*name, at))
            .collect();
        let mut index = NameIndex {
            files,
            entries: names.iter().map(|_| NameEntry::default()).collect(),
            positions,
        };
        for (file_index, file) in files.iter().copied().enumerate() {
            if in_name_graph(file) {
                index.add_file(file_index, file);
            }
        }
        for entry in &mut index.entries {
            entry.defining = entry.definitions.iter().map(Definition::path).collect();
            entry.defining.sort_unstable();
            entry.defining.dedup();
            entry.candidates = Candidates::of(&entry.definitions);
        }
        index
    }

    /// Return the scanned files the index was built from.
    #[must_use]
    pub fn files(&self) -> &'a [&'a FileFacts] {
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
        self.positions.get(name).copied()
    }

    // The definitions of `name` in candidate order, or none for a name that nothing defines.
    pub(crate) fn candidates(&self, name: &str) -> Option<&Candidates<'a>> {
        let entry = &self.entries[self.position(name)?];
        (!entry.definitions.is_empty()).then_some(&entry.candidates)
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
            entry.site_files.push(file_index);
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

    // Reads `files`, the same paths in the same order as the index's scan, and replaces the rows
    // of the file at each `changed` position with its facts there.
    pub(crate) fn update(&mut self, files: &'a [&'a FileFacts], changed: &[usize]) {
        let before = self.files;
        self.files = files;
        for &at in changed {
            self.replace_file(at, before[at], files[at]);
        }
    }

    fn replace_file(&mut self, at: usize, old: &'a FileFacts, new: &'a FileFacts) {
        let (old, new) = (Rows::of(old), Rows::of(new));
        let names: BTreeSet<&'a str> = old.names().chain(new.names()).collect();
        for name in names {
            let position = self.position(name).unwrap_or_else(|| self.added(name));
            let definitions: Vec<Definition<'a>> = (new.symbols.get(name).into_iter().flatten())
                .map(|&symbol| Definition::new(self.files, at, symbol))
                .collect();
            let entry = &mut self.entries[position];
            entry.replace_definitions(at, definitions);
            entry.replace_sites(at, &new, name);
            let (was, is) = (old.spells(name), new.spells(name));
            entry.spelling = entry.spelling + u32::from(is.0) - u32::from(was.0);
            entry.referencing = entry.referencing + u32::from(is.1) - u32::from(was.1);
            if entry.definitions.is_empty() && entry.sites.is_empty() {
                self.positions.remove(name);
            }
        }
    }

    // A name that no file spelled gets a new entry; the positions of the others stay.
    fn added(&mut self, name: &'a str) -> usize {
        self.entries.push(NameEntry::default());
        self.positions.insert(name, self.entries.len() - 1);
        self.entries.len() - 1
    }
}

impl<'a> NameEntry<'a> {
    fn replace_definitions(&mut self, at: usize, definitions: Vec<Definition<'a>>) {
        let start = (self.definitions).partition_point(|held| held.file_index < at);
        let end = (self.definitions).partition_point(|held| held.file_index <= at);
        let (was, is) = (start < end, !definitions.is_empty());
        let path = match (definitions.first(), self.definitions.get(start)) {
            (Some(definition), _) => definition.path(),
            (None, Some(held)) if was => held.path(),
            _ => return,
        };
        self.candidates.replace(path, &definitions);
        self.definitions.splice(start..end, definitions);
        let held = self.defining.binary_search(&path);
        match (was, is, held) {
            (true, false, Ok(found)) => {
                self.defining.remove(found);
            }
            (false, true, Err(free)) => self.defining.insert(free, path),
            _ => {}
        }
    }

    fn replace_sites(&mut self, at: usize, rows: &Rows<'a>, name: &str) {
        let start = self.site_files.partition_point(|file| *file < at);
        let end = self.site_files.partition_point(|file| *file <= at);
        let sites: Vec<Site<'a>> = match (rows.file, rows.refs.get(name)) {
            (Some(file), Some(refs)) => (refs.iter())
                .map(|&reference| Site {
                    file,
                    reference: &file.facts.refs[reference],
                })
                .collect(),
            _ => Vec::new(),
        };
        let count = sites.len();
        self.sites.splice(start..end, sites);
        (self.site_files).splice(start..end, std::iter::repeat_n(at, count));
    }
}

// One name-graph file's symbols and refs by name, each in file order.
#[derive(Default)]
struct Rows<'a> {
    file: Option<&'a FileFacts>,
    symbols: BTreeMap<&'a str, Vec<usize>>,
    refs: BTreeMap<&'a str, Vec<usize>>,
    referenced: BTreeSet<&'a str>,
}

impl<'a> Rows<'a> {
    fn of(file: &'a FileFacts) -> Rows<'a> {
        let mut rows = Rows::default();
        if !in_name_graph(file) {
            return rows;
        }
        rows.file = Some(file);
        for (at, symbol) in file.facts.symbols.iter().enumerate() {
            rows.symbols
                .entry(symbol.name.as_str())
                .or_default()
                .push(at);
        }
        for (at, reference) in file.facts.refs.iter().enumerate() {
            rows.refs
                .entry(reference.name.as_str())
                .or_default()
                .push(at);
            if reference.is_reference() {
                rows.referenced.insert(reference.name.as_str());
            }
        }
        rows
    }

    fn names(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.symbols.keys().chain(self.refs.keys()).copied()
    }

    // Whether the file spells `name` at all, and whether it references it.
    fn spells(&self, name: &str) -> (bool, bool) {
        (self.refs.contains_key(name), self.referenced.contains(name))
    }
}

// A file's spellings arrive together, so a new file index is a new distinct file.
fn count_file(count: &mut u32, last: &mut Option<usize>, file_index: usize) {
    if *last != Some(file_index) {
        *count += 1;
        *last = Some(file_index);
    }
}

// Each name is spelled many times, so the dedup comes before the sort.
fn sorted_names<'a>(files: &[&'a FileFacts]) -> Vec<&'a str> {
    let unique: HashSet<&str> = files
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
    let mut names: Vec<&str> = unique.into_iter().collect();
    names.sort_unstable();
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

#[cfg(test)]
mod canonical;
