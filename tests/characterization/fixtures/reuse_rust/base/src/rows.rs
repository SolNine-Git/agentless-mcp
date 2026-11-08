pub fn compact_rows(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| row.trim())
        .filter(|row| !row.is_empty() && !row.starts_with('#'))
        .map(|row| row.replace('\t', "    "))
        .collect()
}

pub fn first_row(rows: &[String]) -> Option<&String> {
    rows.first()
}
