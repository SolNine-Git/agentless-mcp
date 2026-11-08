pub fn latest_record(records: &[Entry]) -> Option<&Entry> {
    let mut stamped = Vec::with_capacity(records.len());
    for record in records {
        if record.stamp.is_some() {
            stamped.push(record);
        }
    }
    stamped.into_iter().max_by(|left, right| left.stamp.cmp(&right.stamp))
}
