//! The linter rows: for each language, the linter that lints it or the gap it declares.

/// Why a language has no linter row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinterGap {
    /// The row comes in a later unit of work, with its measured cost.
    FollowUp,
    /// The language is tier 2, so no pair check reads it.
    TierTwo,
    /// The language holds data, not code.
    DataLanguage,
}

impl LinterGap {
    /// Returns the reason's name in the rule file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        ["follow_up", "tier_two", "data_language"][self as usize]
    }
}

/// A linter that reads one file's text on stdin and prints its findings as JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Linter {
    /// The linter's name, as the messages show it.
    pub name: &'static str,
    /// The program's path below the repository root, which runs when it is there.
    pub local: &'static str,
    /// The program's name, found on `PATH` when the local program is not there.
    pub program: &'static str,
    /// The arguments; `{path}` stands for the file's path below the repository root.
    pub args: &'static [&'static str],
    /// The files whose presence names a config.
    pub configs: &'static [&'static str],
    /// The TOML file that names a config when it holds the `table` key.
    pub manifest: &'static str,
    /// The keys of the config table in `manifest`, outermost first.
    pub table: &'static [&'static str],
    /// The code of a syntax-error finding. All such findings of one text make one key.
    pub syntax_code: &'static str,
}

/// The linter of one language, or the gap the language declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinterRules {
    /// The language's name.
    pub language: &'static str,
    /// The linter, or why the language has none.
    pub linter: Result<Linter, LinterGap>,
}
