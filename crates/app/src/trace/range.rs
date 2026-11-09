//! The range card: the numbered lines of a `path:start-end` target, cut at the line cap, and the
//! symbols that the shown lines touch.

use std::path::Path;

use facts::line_count;

use super::resolve::LineRange;
use super::{Card, Miss, RANGE_LINES, RANGE_SYMBOLS, RangeTrace};
use crate::search::searched_text;

// The lines are read as search reads them: a binary file has none, and a CRLF line keeps no "\r".
pub(super) fn range_card(root: &Path, range: LineRange) -> Card {
    let LineRange {
        path,
        start,
        end,
        mut symbols,
    } = range;
    let bytes = match io::read_bounded(&root.join(&path)) {
        Ok(bytes) => bytes,
        Err(skipped) => {
            let reason = skipped.to_string();
            return Card::Miss(Miss::NotParsed { path, reason });
        }
    };
    let Some(text) = searched_text(&bytes) else {
        return Card::Miss(Miss::Binary { path });
    };
    let lines = line_count(&text);
    if start > lines {
        return Card::Miss(Miss::RangePastEnd {
            path,
            start,
            end,
            lines,
        });
    }
    let end = end.min(lines);
    let shown = (end - start).min(RANGE_LINES - 1) + 1;
    let text = (text.lines().skip(index(start - 1)).take(index(shown)))
        .map(str::to_owned)
        .collect();
    let symbols_total = symbols.len();
    symbols.truncate(RANGE_SYMBOLS);
    Card::Range(RangeTrace {
        path,
        lines,
        start,
        end,
        text,
        symbols,
        symbols_total,
    })
}

fn index(count: u32) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}
