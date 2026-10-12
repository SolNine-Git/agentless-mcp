use core::num::NonZeroU16;

use crate::generated::languages::{EXTENSIONS, TABLES};
use crate::spec::{self, Spec};
use crate::tables::{Table, Tier};
use crate::{ffi, kinds};

/// One of the 22 languages that the vendored grammars parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Bash,
    C,
    Cpp,
    Csharp,
    Go,
    Hcl,
    Java,
    Javascript,
    Json,
    Kotlin,
    Lua,
    Php,
    Python,
    Ruby,
    Rust,
    Scala,
    Sql,
    Swift,
    Toml,
    Tsx,
    Typescript,
    Yaml,
}

/// A node kind of one grammar, by name: a named and an anonymous kind with one name share it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KindId(pub(crate) u16);

/// A field id of one grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldId(pub(crate) NonZeroU16);

/// A set of node kind ids of one grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindSet {
    words: Vec<u64>,
}

impl Language {
    /// Every language, sorted by name.
    pub const ALL: [Language; 22] = [
        Language::Bash,
        Language::C,
        Language::Cpp,
        Language::Csharp,
        Language::Go,
        Language::Hcl,
        Language::Java,
        Language::Javascript,
        Language::Json,
        Language::Kotlin,
        Language::Lua,
        Language::Php,
        Language::Python,
        Language::Ruby,
        Language::Rust,
        Language::Scala,
        Language::Sql,
        Language::Swift,
        Language::Toml,
        Language::Tsx,
        Language::Typescript,
        Language::Yaml,
    ];

    /// Return the language's name, such as `csharp`.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.table().name
    }

    /// Return the language named `name`, compared case-sensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Language> {
        Self::ALL
            .into_iter()
            .find(|language| language.name() == name)
    }

    /// Return the language that the last suffix of the path's final component selects, compared
    /// case-sensitively.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Language> {
        let name = std::path::Path::new(path).file_name()?.to_str()?;
        let dot = name
            .rfind('.')
            .filter(|dot| *dot > 0 && *dot + 1 < name.len())?;
        let suffix = &name[dot..];
        let found = EXTENSIONS
            .binary_search_by(|(known, _)| (*known).cmp(suffix))
            .ok()?;
        Some(EXTENSIONS[found].1)
    }

    /// Return the suffixes that select the language, each with its dot, sorted.
    #[must_use]
    pub fn extensions(self) -> &'static [&'static str] {
        self.table().extensions
    }

    /// Return the language's support tier.
    #[must_use]
    pub fn tier(self) -> Tier {
        self.table().tier
    }

    /// Return the one-line source that the capabilities probe parses.
    #[must_use]
    pub fn probe_sample(self) -> &'static str {
        self.table().probe
    }

    /// Return the measurement tables, or None for a tier-2 language.
    #[must_use]
    pub fn spec(self) -> Option<&'static Spec> {
        spec::spec(self)
    }

    pub(crate) fn table(self) -> &'static Table {
        &TABLES[self as usize]
    }

    /// Return the ABI version of the compiled grammar.
    #[must_use]
    pub fn abi_version(self) -> usize {
        self.grammar().abi_version()
    }

    /// Return the kinds whose name is in `names`, named or anonymous.
    #[must_use]
    pub fn kinds(self, names: &[&str]) -> KindSet {
        let table = kinds::table(self);
        let mut set = KindSet {
            words: vec![0; table.len().div_ceil(64)],
        };
        for id in names.iter().filter_map(|name| table.id_by_name(name)) {
            set.insert(id);
        }
        set
    }

    /// Return the id of the field `name`, or None when the grammar has no such field.
    #[must_use]
    pub fn field(self, name: &str) -> Option<FieldId> {
        self.grammar().field_id_for_name(name).map(FieldId)
    }

    pub(crate) fn grammar(self) -> tree_sitter::Language {
        tree_sitter::Language::new(ffi::LANGUAGE_FNS[self as usize])
    }
}

impl KindSet {
    /// Return whether the set holds `kind`.
    #[must_use]
    pub fn contains(&self, kind: KindId) -> bool {
        let id = usize::from(kind.0);
        self.words
            .get(id / 64)
            .is_some_and(|word| word & (1 << (id % 64)) != 0)
    }

    /// Return whether the set holds no kind.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    fn insert(&mut self, id: u16) {
        let id = usize::from(id);
        self.words[id / 64] |= 1 << (id % 64);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{KindId, Language};
    use crate::parse;

    #[test]
    fn names_are_sorted_and_round_trip() {
        let names: Vec<&str> = Language::ALL
            .iter()
            .map(|language| language.name())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        for language in Language::ALL {
            assert_eq!(Language::from_name(language.name()), Some(language));
            assert_eq!(format!("{language:?}").to_lowercase(), language.name());
        }
        assert_eq!(Language::from_name("Python"), None);
        assert_eq!(Language::from_name("c_sharp"), None);
    }

    #[test]
    fn a_kind_set_holds_the_kinds_it_names() {
        let parsed = parse(Language::Python, Arc::from("if x:\n    pass\n")).expect("it parses");
        let set = Language::Python.kinds(&["identifier", "if"]);
        let held: Vec<&str> = parsed
            .descendants(parsed.root())
            .filter(|node| set.contains(parsed.kind(*node)))
            .map(|node| parsed.kind_name(node))
            .collect();
        assert_eq!(held, ["if", "identifier"]);
        assert!(!set.contains(KindId(u16::MAX)));
        assert!(Language::Python.kinds(&["no_such_kind"]).is_empty());
    }

    #[test]
    fn every_scala_kind_keeps_the_name_that_tree_sitter_gives_it() {
        let parsed = parse(Language::Scala, Arc::from("object A { f(1) }\n")).expect("it parses");
        let open = Language::Scala.kinds(&["("]);
        let found = parsed
            .descendants(parsed.root())
            .find(|node| open.contains(parsed.kind(*node)))
            .expect("an open parenthesis");
        assert_eq!(parsed.text(found), "(");
        assert!(
            !Language::Scala
                .kinds(&["arguments"])
                .contains(parsed.kind(found))
        );
    }

    #[test]
    fn fields_resolve_by_name() {
        assert!(Language::Python.field("name").is_some());
        assert!(Language::Python.field("no_such_field").is_none());
    }
}
