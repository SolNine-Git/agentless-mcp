use tree_sitter::{QueryCapture, QueryCursor, StreamingIterator};

use crate::{Language, NodeId, Parsed};

/// A tree-sitter query compiled against one grammar.
pub struct Query {
    language: Language,
    query: tree_sitter::Query,
}

/// Why a query did not compile: where tree-sitter stopped, and its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct QueryError {
    pub row: usize,
    pub column: usize,
    pub offset: usize,
    pub message: String,
}

/// Why a query run returned no result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueryRunError {
    #[error("the query exceeded its match limit, so its results are incomplete")]
    MatchLimit,
    #[error("the query is for {query:?}, and the tree is {tree:?}")]
    OtherLanguage { query: Language, tree: Language },
}

/// One match of a query: its pattern and its captures, in tree-sitter's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub pattern: u32,
    pub captures: Vec<Capture>,
}

/// One captured node, with the index of its capture name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capture {
    pub index: u32,
    pub node: NodeId,
}

impl Query {
    /// Compile `source` against the grammar of `language`.
    ///
    /// # Errors
    /// Returns `QueryError` when tree-sitter rejects the query.
    pub fn new(language: Language, source: &str) -> Result<Query, QueryError> {
        let query =
            tree_sitter::Query::new(&language.grammar(), source).map_err(|error| QueryError {
                row: error.row,
                column: error.column,
                offset: error.offset,
                message: error.to_string(),
            })?;
        Ok(Query { language, query })
    }

    /// Return the language the query was compiled for.
    #[must_use]
    pub fn language(&self) -> Language {
        self.language
    }

    /// Return the capture names, indexed as `Capture::index` indexes them.
    #[must_use]
    pub fn capture_names(&self) -> &[&str] {
        self.query.capture_names()
    }

    /// Return the number of patterns in the query.
    #[must_use]
    pub fn pattern_count(&self) -> usize {
        self.query.pattern_count()
    }

    /// Return the byte offset in the query source where pattern `index` starts.
    ///
    /// # Panics
    /// Panics when `index` is not below the pattern count.
    #[must_use]
    pub fn start_byte_for_pattern(&self, index: usize) -> usize {
        self.query.start_byte_for_pattern(index)
    }
}

impl Parsed {
    /// Run `query` from the root, text predicates applied, and return every match in order.
    ///
    /// # Errors
    /// Returns `QueryRunError` when the query is for another language or exceeds the match limit.
    pub fn matches(&self, query: &Query) -> Result<Vec<Match>, QueryRunError> {
        self.matches_within(query, u32::MAX)
    }

    /// Run `query` from the root, text predicates applied, and return every capture in order.
    ///
    /// # Errors
    /// Returns `QueryRunError` when the query is for another language or exceeds the match limit.
    pub fn captures(&self, query: &Query) -> Result<Vec<Capture>, QueryRunError> {
        self.same_language(query)?;
        let mut cursor = QueryCursor::new();
        let mut found = Vec::new();
        let mut captures = cursor.captures(
            &query.query,
            self.tree.root_node(),
            self.source().as_bytes(),
        );
        while let Some((matched, index)) = captures.next() {
            found.push(self.capture(&matched.captures[*index]));
        }
        within_limit(&cursor, found)
    }

    pub(crate) fn matches_within(
        &self,
        query: &Query,
        limit: u32,
    ) -> Result<Vec<Match>, QueryRunError> {
        self.same_language(query)?;
        let mut cursor = QueryCursor::new();
        cursor.set_match_limit(limit);
        let mut found = Vec::new();
        let mut matches = cursor.matches(
            &query.query,
            self.tree.root_node(),
            self.source().as_bytes(),
        );
        while let Some(matched) = matches.next() {
            found.push(Match {
                pattern: u32::try_from(matched.pattern_index).unwrap_or(u32::MAX),
                captures: matched
                    .captures
                    .iter()
                    .map(|capture| self.capture(capture))
                    .collect(),
            });
        }
        within_limit(&cursor, found)
    }

    fn same_language(&self, query: &Query) -> Result<(), QueryRunError> {
        if query.language == self.language() {
            Ok(())
        } else {
            Err(QueryRunError::OtherLanguage {
                query: query.language,
                tree: self.language(),
            })
        }
    }

    fn capture(&self, capture: &QueryCapture) -> Capture {
        Capture {
            index: capture.index,
            node: self
                .node_id(capture.node)
                .expect("a captured node lies in the tree it was captured from"),
        }
    }
}

fn within_limit<T>(cursor: &QueryCursor, found: Vec<T>) -> Result<Vec<T>, QueryRunError> {
    if cursor.did_exceed_match_limit() {
        Err(QueryRunError::MatchLimit)
    } else {
        Ok(found)
    }
}
