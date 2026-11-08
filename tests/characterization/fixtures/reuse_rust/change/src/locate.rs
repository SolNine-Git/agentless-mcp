pub fn span_at(line: u32, spans: &[(u32, u32)]) -> Option<usize> {
    let after = spans.partition_point(|span| span.0 <= line);
    let index = after.checked_sub(1)?;
    let (_, end) = spans.get(index).copied()?;
    (end >= line).then_some(index)
}
