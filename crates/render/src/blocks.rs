//! The text parts that several renderers share: a list of items as blocks of one text, and the
//! " -- detail" suffix of a line.

// The block of each item joined by `separator`, with a final line break, or the `empty` line
// when there are no items.
pub(crate) fn blocks_or<T>(
    items: &[T],
    block: impl Fn(&T) -> String,
    separator: &str,
    empty: &str,
) -> String {
    if items.is_empty() {
        return format!("{empty}\n");
    }
    let blocks: Vec<String> = items.iter().map(block).collect();
    blocks.join(separator) + "\n"
}

// " -- {detail}", or nothing for an empty detail.
pub(crate) fn detail_suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(" -- {detail}")
    }
}
