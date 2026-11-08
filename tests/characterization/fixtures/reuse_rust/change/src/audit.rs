pub fn gather_failures(report: &Report, limit: usize) -> Vec<String> {
    let mut found: Vec<String> = report
        .entries
        .iter()
        .filter(|entry| entry.severity == Severity::Error && !entry.waived && entry.code.is_some())
        .map(|entry| entry.message.clone())
        .collect();
    found.sort_by_key(|message| message.to_lowercase());
    found.truncate(limit);
    found
}
