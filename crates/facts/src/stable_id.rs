use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use syntax::Language;
use text::CodePointString;

use crate::values::{Rationale, Symbol};

const QUALNAME_SEPARATOR: &str = "::";
// The builder and the parser agree on this text: one adds it, the other refuses any id with it.
const RATIONALE_MARKER: &str = "::rationale@";
// More digits than this after `#` are part of a name, not an ordinal.
const ORDINAL_MAX_DIGITS: usize = 6;

/// A stable id: a language prefix, a repository-relative path, and a qualified name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StableId {
    prefix: String,
    path: String,
    qualname: String,
}

/// Why a text is not a stable id.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StableIdError {
    #[error(
        "not a stable id: {}; this is a rationale handle, which names a comment rather than a \
         symbol. Expand the symbol it hangs off instead.",
        quoted(.text)
    )]
    RationaleHandle { text: String },
    #[error(
        "not a stable id: {}; expected <langprefix>:<relpath>::<qualname>, for example \
         py:src/app/svc.py::Invoice.total",
        quoted(.text)
    )]
    Malformed { text: String },
}

impl StableId {
    /// Return the id of the three parts.
    #[must_use]
    pub fn new(prefix: &str, path: &str, qualname: &str) -> StableId {
        StableId {
            prefix: prefix.to_owned(),
            path: path.to_owned(),
            qualname: qualname.to_owned(),
        }
    }

    /// Return the id of `symbol`, which the file at `path` declares in `language`.
    #[must_use]
    pub fn of(language: Language, path: &str, symbol: &Symbol) -> StableId {
        StableId::new(language.extraction().prefix, path, &symbol.id_qualname())
    }

    /// Return the language prefix, such as `py`.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Return the repository-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the qualified name, with its ordinal when it has one.
    #[must_use]
    pub fn qualname(&self) -> &str {
        &self.qualname
    }
}

impl fmt::Display for StableId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}{QUALNAME_SEPARATOR}{}",
            self.prefix, self.path, self.qualname
        )
    }
}

impl FromStr for StableId {
    type Err = StableIdError;

    fn from_str(text: &str) -> Result<StableId, StableIdError> {
        if text.contains(RATIONALE_MARKER) {
            return Err(StableIdError::RationaleHandle {
                text: text.to_owned(),
            });
        }
        match parts(text) {
            Some([prefix, path, qualname]) => Ok(StableId::new(prefix, path, qualname)),
            None => Err(StableIdError::Malformed {
                text: text.to_owned(),
            }),
        }
    }
}

impl Symbol {
    /// Return the qualified name that the id spells: with `#N` for the Nth symbol of the file
    /// that shares it, from the second on.
    #[must_use]
    pub fn id_qualname(&self) -> String {
        let qualname = self.qualname();
        if self.duplicate_index == 0 {
            qualname
        } else {
            format!("{qualname}#{}", u64::from(self.duplicate_index) + 1)
        }
    }
}

/// Split a qualified name into its base and its 0-based collision ordinal; a name with no ordinal
/// returns itself and 0.
#[must_use]
pub fn split_ordinal(qualname: &str) -> (&str, u32) {
    let Some(hash) = qualname.rfind('#') else {
        return (qualname, 0);
    };
    let tail = &qualname[hash + 1..];
    // The end of the name may sit before one final line feed, so `f#12\n` still carries the
    // ordinal 12.
    let digits = tail.strip_suffix('\n').unwrap_or(tail);
    let ordinal = (1..=ORDINAL_MAX_DIGITS)
        .contains(&digits.len())
        .then(|| digits.parse::<u32>().ok())
        .flatten();
    match ordinal {
        Some(ordinal) if digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            (&qualname[..hash], ordinal.saturating_sub(1))
        }
        _ => (qualname, 0),
    }
}

/// Return the handle of one rationale below `symbol`: its id, `::rationale@`, the line, and
/// `#N` for the Nth rationale on that line from the second on.
#[must_use]
pub fn rationale_handle(
    language: Language,
    path: &str,
    symbol: &Symbol,
    rationale: &Rationale,
) -> String {
    let id = StableId::of(language, path, symbol);
    let line = rationale.line;
    match rationale.duplicate_index {
        0 => format!("{id}{RATIONALE_MARKER}{line}"),
        index => format!("{id}{RATIONALE_MARKER}{line}#{}", u64::from(index) + 1),
    }
}

// Give each symbol the count of earlier symbols with its qualified name, ordered by line and
// then by emission, so that the ordinals follow the source.
pub(crate) fn disambiguate(mut symbols: Vec<Symbol>) -> Vec<Symbol> {
    let mut order: Vec<usize> = (0..symbols.len()).collect();
    order.sort_by_key(|&position| symbols[position].line);
    let mut seen: HashMap<String, u32> = HashMap::new();
    for position in order {
        let count = seen.entry(symbols[position].qualname()).or_insert(0);
        symbols[position].duplicate_index = *count;
        *count += 1;
    }
    symbols
}

fn parts(text: &str) -> Option<[&str; 3]> {
    let (prefix, rest) = text.split_once(':')?;
    let (path, qualname) = rest.split_once(QUALNAME_SEPARATOR)?;
    let parts = [prefix, path, qualname];
    parts.iter().all(|part| well_formed(part)).then_some(parts)
}

// A component of pure whitespace addresses nothing, and a control character would break the
// line-oriented output that quotes the component.
fn well_formed(part: &str) -> bool {
    !text::strip(part).is_empty() && !part.chars().any(|c| matches!(c, '\0'..='\x1f' | '\x7f'))
}

fn quoted(text: &str) -> String {
    CodePointString::from(text).quoted()
}

#[cfg(test)]
mod tests {
    use syntax::Language;

    use super::{StableId, StableIdError, rationale_handle, split_ordinal};
    use crate::values::{Rationale, RationaleKind, Symbol, SymbolKind};

    fn symbol(name: &str, parent: &str, line: u32) -> Symbol {
        Symbol {
            name: name.to_owned(),
            kind: SymbolKind::Function,
            line,
            end_line: line,
            signature: String::new(),
            parent: parent.to_owned(),
            decorated: false,
            bases: Vec::new(),
            is_public: true,
            rationales: Vec::new(),
            duplicate_index: 0,
        }
    }

    #[test]
    fn an_id_displays_its_three_parts_and_parses_back() {
        let id = StableId::new("py", "src/app/svc.py", "Invoice.total");
        assert_eq!(id.to_string(), "py:src/app/svc.py::Invoice.total");
        assert_eq!("py:src/app/svc.py::Invoice.total".parse(), Ok(id));
        let id: StableId = "a:b::c::d".parse().expect("a qualified name can hold ::");
        assert_eq!((id.prefix(), id.path(), id.qualname()), ("a", "b", "c::d"));
    }

    #[test]
    fn a_handle_or_a_malformed_text_is_refused_with_the_reference_text() {
        let handle = "py:a.py::f::rationale@3".parse::<StableId>().unwrap_err();
        assert!(matches!(handle, StableIdError::RationaleHandle { .. }));
        assert!(
            handle
                .to_string()
                .starts_with("not a stable id: 'py:a.py::f::rationale@3'; ")
        );
        for text in [
            "",
            "py",
            "py:a.py",
            ":a.py::f",
            "py: ::f",
            "py:a.py::\u{3000}",
            "py:a\n::f",
        ] {
            let error = text.parse::<StableId>().unwrap_err();
            assert!(matches!(error, StableIdError::Malformed { .. }), "{text:?}");
        }
    }

    #[test]
    fn an_ordinal_is_one_to_six_ascii_digits_at_the_end() {
        let cases = [
            ("f", ("f", 0)),
            ("f#2", ("f", 1)),
            ("f#0", ("f", 0)),
            ("f#000002", ("f", 1)),
            ("f#1234567", ("f#1234567", 0)),
            ("f#12\n", ("f", 11)),
            ("a#1#2", ("a#1", 1)),
            ("f#", ("f#", 0)),
            ("f#-1", ("f#-1", 0)),
            ("f#\u{663}", ("f#\u{663}", 0)),
        ];
        for (text, expected) in cases {
            assert_eq!(split_ordinal(text), expected, "{text:?}");
        }
    }

    #[test]
    fn a_rationale_handle_carries_the_line_and_its_ordinal() {
        let mut symbol = symbol("total", "Invoice", 4);
        symbol.duplicate_index = 1;
        let mut rationale = Rationale {
            kind: RationaleKind::Note,
            text: String::new(),
            line: 7,
            citations: Vec::new(),
            duplicate_index: 0,
        };
        let handle = rationale_handle(Language::Python, "a.py", &symbol, &rationale);
        assert_eq!(handle, "py:a.py::Invoice.total#2::rationale@7");
        rationale.duplicate_index = 2;
        let handle = rationale_handle(Language::Python, "a.py", &symbol, &rationale);
        assert_eq!(handle, "py:a.py::Invoice.total#2::rationale@7#3");
    }
}
