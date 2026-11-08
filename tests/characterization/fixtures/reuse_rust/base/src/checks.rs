pub fn collect_errors(report: &Report) -> Vec<String> {
    let mut found: Vec<String> = report
        .entries
        .iter()
        .filter(|entry| entry.severity == Severity::Error && !entry.waived)
        .map(|entry| entry.message.clone())
        .collect();
    found.sort_by_key(|message| message.to_lowercase());
    found
}
