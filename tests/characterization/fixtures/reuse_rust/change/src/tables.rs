pub fn compact_rows(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| row.trim())
        .filter(|row| !row.is_empty() && !row.starts_with('#'))
        .map(|row| row.replace('\t', "    "))
        .collect()
}
