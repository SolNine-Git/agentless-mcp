//! The comment rule rows: for each language, the rules of each group or the gap it declares.

/// A group of comment rules. A language has rows in a group or declares a gap in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommentGroup {
    /// The comment forms, and the banner, block and run rules.
    Runs,
    /// The doc rules.
    Docs,
    /// The lint-suppression rule.
    Suppressions,
}

impl CommentGroup {
    /// Every group, in the order of a row.
    pub const ALL: [CommentGroup; 3] = [Self::Runs, Self::Docs, Self::Suppressions];

    /// Returns the group's name in the rule file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        ["runs", "docs", "suppressions"][self as usize]
    }
}

/// Why a language has no rows in a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentGap {
    /// The rows come in a later unit of work.
    FollowUp,
    /// The language writes doc comments as plain comments, and no rule attaches them to an item.
    DocAttachment,
    /// The language is tier 2, so no pair check reads it.
    TierTwo,
    /// The language holds data, not code.
    DataLanguage,
}

impl CommentGap {
    /// Returns the reason's name in the rule file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        ["follow_up", "doc_attachment", "tier_two", "data_language"][self as usize]
    }
}

/// The rows of one group, or the gap the language declares in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Declared<T> {
    /// The group's rows.
    Rules(T),
    /// The declared gap.
    Gap(CommentGap),
}

/// The part of a comment that the patterns match and the key holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    /// The whole comment, as written.
    Comment,
    /// The comment's first line, with whitespace stripped.
    FirstLine,
}

/// The comment forms of one language and its banner, block and run rules, as regular expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunRules {
    /// Matches a line comment.
    pub line: &'static str,
    /// Matches a block comment, or `None` for a language with none.
    pub block: Option<&'static str>,
    /// Matches a doc comment, which the block and run rules never count.
    pub doc: Option<&'static str>,
    /// The text the patterns match and the key holds.
    pub head: Head,
    /// Matches a directive, which no rule reports and no run counts.
    pub directive: Option<&'static str>,
    /// Matches a banner comment.
    pub banner: &'static str,
}

/// The doc rules of one language, as regular expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocRules {
    /// Python docstrings: none on a private function, and no section in a public one.
    Docstring {
        /// Matches the name of a function that is not public.
        private: &'static str,
        /// Matches a section heading in a docstring.
        sections: &'static str,
    },
    /// The tags of a `/** */` comment that the typed signature carries.
    Jsdoc {
        /// Matches a tag, with the tag name as group 1.
        tags: &'static str,
        /// Each tag spelling and the name it counts as.
        aliases: &'static [(&'static str, &'static str)],
    },
    /// Rust doc comments: none on a non-public or trait-impl item, and no section in any.
    RustItems {
        /// Matches the head of the item after a doc comment, with groups `visibility`, `kind`, `name`.
        item: &'static str,
        /// The one visibility that counts as public.
        public: &'static str,
        /// The text in an attribute that makes a `macro_rules!` macro public.
        export: &'static str,
        /// Matches an attribute whose item publishes its doc comments to callers at run time.
        facing: &'static str,
        /// Matches a section heading in a doc comment.
        sections: &'static str,
    },
}

/// Where a language writes a lint suppression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suppressed {
    /// In a comment, which the pattern matches by its head.
    Comment,
    /// In an attribute, which the pattern matches by its body with whitespace collapsed.
    Attribute,
}

/// The lint-suppression rule of one language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuppressionRules {
    /// Where the suppression is written.
    pub on: Suppressed,
    /// The regular expression that matches a suppression.
    pub pattern: &'static str,
}

/// The comment rules of one language: the rows or the gap of each group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentRules {
    /// The language's name.
    pub language: &'static str,
    /// The comment forms, and the banner, block and run rules.
    pub runs: Declared<RunRules>,
    /// The doc rules.
    pub docs: Declared<DocRules>,
    /// The lint-suppression rule.
    pub suppressions: Declared<SuppressionRules>,
}

impl CommentRules {
    /// Returns the gap that the language declares in `group`, or `None` when it has rows there.
    #[must_use]
    pub fn gap(&self, group: CommentGroup) -> Option<CommentGap> {
        match (group, self.runs, self.docs, self.suppressions) {
            (CommentGroup::Runs, Declared::Gap(gap), _, _)
            | (CommentGroup::Docs, _, Declared::Gap(gap), _)
            | (CommentGroup::Suppressions, _, _, Declared::Gap(gap)) => Some(gap),
            _ => None,
        }
    }
}
