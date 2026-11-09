//! The range card's text: the header, the numbered lines, the line that names the next range when
//! the cap cut this one, and the symbols that the shown lines touch.

use app::ROW_INDENT;
use app::trace::{RangeTrace, Tally};
use facts::line_prefix;
use prompts::messages::{
    TRACE_RANGE_CUT, TRACE_RANGE_HEADER, TRACE_RANGE_SYMBOLS, TRACE_RANGE_SYMBOLS_NONE,
};
use text::one_line;

use super::{card_row, unlisted};

pub(super) fn range_lines(range: &RangeTrace) -> Vec<String> {
    let path = one_line(&range.path);
    let header = (
        &*path,
        range.start as usize,
        range.end as usize,
        range.lines as usize,
    );
    let mut lines = vec![TRACE_RANGE_HEADER.render(header)];
    lines.extend(
        (range.text.iter().zip(range.start..))
            .map(|(text, number)| format!("{ROW_INDENT}{}{}", line_prefix(number), one_line(text))),
    );
    if let Some((start, end)) = range.next() {
        let target = format!("{path}:{start}-{end}");
        let next = (end - start + 1) as usize;
        let total = (range.end - range.start + 1) as usize;
        lines.push(TRACE_RANGE_CUT.render((&target, range.text.len(), next, total)));
    }
    if range.symbols_total == 0 {
        lines.push(TRACE_RANGE_SYMBOLS_NONE.to_owned());
        return lines;
    }
    lines.push(TRACE_RANGE_SYMBOLS.render(range.symbols_total));
    lines.extend(range.symbols.iter().map(card_row));
    lines.extend(unlisted(
        range.symbols.len(),
        Tally::Exact(range.symbols_total),
    ));
    lines
}
