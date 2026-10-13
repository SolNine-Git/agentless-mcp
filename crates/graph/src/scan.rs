//! One scanned file, as every graph builder reads it.

use facts::Facts;
use syntax::Language;

/// One scanned file: its repository-relative path, its language, its line count and its facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    pub path: String,
    pub language: Language,
    pub line_count: u32,
    pub facts: Facts,
}

/// Return whether a file's symbols and spellings join the name graph: a data file's keys do not.
#[must_use]
pub fn in_name_graph(file: &FileFacts) -> bool {
    !matches!(
        file.language,
        Language::Json | Language::Toml | Language::Yaml
    )
}
