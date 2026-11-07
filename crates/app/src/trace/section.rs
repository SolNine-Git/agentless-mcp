//! The sections of the trace cards, which `more` names one of, and the card kinds that hold them.

/// One section of a card, which `more` shows in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The definitions of a name with more than one.
    Candidates,
    /// The sites that reference a symbol.
    Callers,
    /// The symbols that a symbol references.
    Callees,
    /// The test files that reference a symbol, or that import a file.
    Tests,
    /// The symbols that a symbol's callers also reference.
    Similar,
    /// The commits that touched a symbol's lines.
    History,
    /// The numbered body of a symbol.
    Body,
    /// The declarations of a file.
    Declares,
    /// The files that a file imports.
    Imports,
    /// The files that import a file.
    Importers,
    /// The import cycles that a file is in.
    Cycles,
    /// The entries of a folder.
    Tree,
}

const SECTION_NAMES: [&str; 12] = [
    "candidates",
    "callers",
    "callees",
    "tests",
    "similar",
    "history",
    "body",
    "declares",
    "imports",
    "importers",
    "cycles",
    "tree",
];

impl Section {
    /// Every section, in the order of the `more` enum.
    pub const ALL: [Section; 12] = [
        Section::Candidates,
        Section::Callers,
        Section::Callees,
        Section::Tests,
        Section::Similar,
        Section::History,
        Section::Body,
        Section::Declares,
        Section::Imports,
        Section::Importers,
        Section::Cycles,
        Section::Tree,
    ];

    /// Returns the wire value, such as `callers`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        SECTION_NAMES[self as usize]
    }

    /// Returns the section whose wire value is `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Section> {
        let at = SECTION_NAMES.iter().position(|known| *known == name)?;
        Some(Section::ALL[at])
    }
}

/// The kind of a card, which decides the sections it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// A function, method, class or other symbol.
    Symbol,
    /// A file.
    File,
    /// A folder.
    Folder,
    /// A name with several definitions.
    Candidates,
    /// A target that matched nothing.
    Miss,
}

const SYMBOL_SECTIONS: [Section; 6] = [
    Section::Callers,
    Section::Callees,
    Section::Tests,
    Section::Similar,
    Section::History,
    Section::Body,
];
const FILE_SECTIONS: [Section; 5] = [
    Section::Declares,
    Section::Imports,
    Section::Importers,
    Section::Tests,
    Section::Cycles,
];

const KIND_NAMES: [&str; 5] = ["symbol", "file", "folder", "candidates", "miss"];

impl CardKind {
    /// Returns the wire value, such as `file`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        KIND_NAMES[self as usize]
    }

    /// Returns the sections that a card of this kind holds, in card order.
    #[must_use]
    pub fn sections(self) -> &'static [Section] {
        match self {
            CardKind::Symbol => &SYMBOL_SECTIONS,
            CardKind::File => &FILE_SECTIONS,
            CardKind::Folder => &[Section::Tree],
            CardKind::Candidates => &[Section::Candidates],
            CardKind::Miss => &[],
        }
    }
}
