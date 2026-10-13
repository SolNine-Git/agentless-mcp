//! Which symbol owns a line: the innermost containing span, by the latest start line.

use facts::Symbol;

/// Return the innermost symbol whose span holds `line`: the latest start wins, and the symbol
/// declared first wins a tie.
#[must_use]
pub fn enclosing_symbol(symbols: &[Symbol], line: u32) -> Option<&Symbol> {
    let mut held: Option<&Symbol> = None;
    for symbol in symbols {
        if symbol.line <= line && line <= symbol.end_line && wins(symbol, held) {
            held = Some(symbol);
        }
    }
    held
}

// A later start is a deeper span; an equal start keeps the symbol declared first.
fn wins(candidate: &Symbol, held: Option<&Symbol>) -> bool {
    held.is_none_or(|held| held.line < candidate.line)
}

fn claim(cells: &mut [Option<usize>], symbols: &[Symbol], position: usize) {
    for cell in cells {
        if wins(&symbols[position], cell.map(|held| &symbols[held])) {
            *cell = Some(position);
        }
    }
}

/// The owner of every covered line of one file, by the rule of [`enclosing_symbol`].
#[derive(Clone, Debug)]
pub struct LineOwners<'a> {
    symbols: &'a [Symbol],
    first: u32,
    owners: Vec<Option<usize>>,
}

impl<'a> LineOwners<'a> {
    /// Build the owner table of one file's symbols.
    #[must_use]
    pub fn build(symbols: &'a [Symbol]) -> LineOwners<'a> {
        let spans = symbols
            .iter()
            .filter(|symbol| symbol.line <= symbol.end_line);
        let first = spans.clone().map(|symbol| symbol.line).min().unwrap_or(0);
        let last = spans.map(|symbol| symbol.end_line).max().unwrap_or(0);
        let mut owners = vec![None; (last.saturating_sub(first) + 1) as usize];
        for (position, symbol) in symbols.iter().enumerate() {
            if symbol.line <= symbol.end_line {
                let lines = (symbol.line - first) as usize..=(symbol.end_line - first) as usize;
                claim(&mut owners[lines], symbols, position);
            }
        }
        LineOwners {
            symbols,
            first,
            owners,
        }
    }

    /// Return the symbol that owns `line`, if any.
    #[must_use]
    pub fn owner(&self, line: u32) -> Option<&'a Symbol> {
        self.owner_position(line)
            .map(|position| &self.symbols[position])
    }

    pub(crate) fn owner_position(&self, line: u32) -> Option<usize> {
        let offset = line.checked_sub(self.first)?;
        self.owners.get(offset as usize).copied().flatten()
    }
}
