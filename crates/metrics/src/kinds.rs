use syntax::{KindSet, Language, NodeId, Parsed};

// The kinds that the metrics rules name in code rather than in the language tables.
pub(crate) struct Kinds {
    pub(crate) string: KindSet,
    pub(crate) string_fragment: KindSet,
    pub(crate) tuple_expression: KindSet,
}

impl Kinds {
    pub(crate) fn new(language: Language) -> Self {
        Self {
            string: language.kinds(&["string"]),
            string_fragment: language.kinds(&["string_fragment"]),
            tuple_expression: language.kinds(&["tuple_expression"]),
        }
    }

    // A plain string is a string literal with no bytes or format prefix.
    pub(crate) fn is_plain_string(&self, parsed: &Parsed, node: NodeId) -> bool {
        self.string.contains(parsed.kind(node)) && !bytes_or_format(parsed.text(node))
    }
}

// Only b, B, f and F lower-case to b or f, so an ASCII test reads the lower-cased prefix exactly.
fn bytes_or_format(literal: &str) -> bool {
    literal
        .chars()
        .take_while(|&c| c != '"' && c != '\'')
        .any(|c| matches!(c, 'b' | 'B' | 'f' | 'F'))
}
