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
    joins_name_graph(file.language)
}

/// Return whether the symbols and spellings of a file in `language` join the name graph.
#[must_use]
pub fn joins_name_graph(language: Language) -> bool {
    !matches!(language, Language::Json | Language::Toml | Language::Yaml)
}

#[cfg(test)]
mod tests {
    use syntax::Language;

    use super::joins_name_graph;

    #[test]
    fn only_the_data_languages_stay_out_of_the_name_graph() {
        let out: Vec<Language> = Language::ALL
            .into_iter()
            .filter(|&language| !joins_name_graph(language))
            .collect();
        assert_eq!(out, [Language::Json, Language::Toml, Language::Yaml]);
    }
}
