pub fn newest_entry(entries: &[Entry]) -> Option<&Entry> {
    entries
        .iter()
        .filter(|entry| entry.stamp.is_some())
        .max_by(|left, right| left.stamp.cmp(&right.stamp))
}
