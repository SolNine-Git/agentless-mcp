//! The identifiers that name a file's symbols where they are declared, which reference nothing.

use std::collections::BTreeMap;

use crate::{Ref, Role, Symbol};

/// The lines of one file where a symbol has its name, and how often each line spells it.
#[derive(Clone, Debug, Default)]
pub struct Declared<'a> {
    lines: BTreeMap<(&'a str, u32), Spelled>,
}

// On one line, of one name: the symbols named there, the spellings in the declaration role, and
// every spelling.
#[derive(Clone, Copy, Debug, Default)]
struct Spelled {
    symbols: usize,
    declarations: usize,
    spellings: usize,
}

impl<'a> Declared<'a> {
    /// Collect the name lines of `symbols` and the spellings in `refs`, both of one file.
    #[must_use]
    pub fn of(symbols: &'a [Symbol], refs: &'a [Ref]) -> Declared<'a> {
        let mut lines: BTreeMap<(&'a str, u32), Spelled> = BTreeMap::new();
        for symbol in symbols {
            let key = (symbol.name.as_str(), symbol.name_line);
            lines.entry(key).or_default().symbols += 1;
        }
        let declaring = refs
            .iter()
            .filter(|reference| reference.role == Role::Declaration);
        for reference in declaring {
            let key = (reference.name.as_str(), reference.line);
            lines.entry(key).or_default().declarations += 1;
        }
        for reference in refs {
            let key = (reference.name.as_str(), reference.line);
            if let Some(spelled) = lines.get_mut(&key) {
                spelled.spellings += 1;
            }
        }
        Declared { lines }
    }

    /// Return whether a symbol of the file has the name `name` and its name on `line`.
    #[must_use]
    pub fn holds(&self, name: &str, line: u32) -> bool {
        self.lines
            .get(&(name, line))
            .is_some_and(|spelled| spelled.symbols > 0)
    }

    /// Return whether `reference` has the declaration role, or spells the name of a symbol on
    /// that symbol's name line.
    #[must_use]
    pub fn declares(&self, reference: &Ref) -> bool {
        // Every spelling on the line counts: the extra ones need column-aware resolution.
        reference.role == Role::Declaration || self.holds(&reference.name, reference.line)
    }

    /// Return whether every spelling of `name` on `line` declares: the line spells the name no
    /// more times than it declares it, so the line holds no reference to it.
    #[must_use]
    pub fn only_declares(&self, name: &str, line: u32) -> bool {
        self.lines.get(&(name, line)).is_some_and(|spelled| {
            let declared = spelled.symbols.max(spelled.declarations);
            spelled.spellings <= declared
        })
    }
}
