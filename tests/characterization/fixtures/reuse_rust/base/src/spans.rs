pub fn enclosing(line: u32, spans: &[(u32, u32)]) -> Option<usize> {
    let at = spans.partition_point(|(start, _)| *start <= line);
    at.checked_sub(1).filter(|&index| spans[index].1 >= line)
}
