pub fn format_grid(rows: &[Vec<String>], title: &str) -> String {
    let columns = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|at| rows.iter().map(|row| row[at].chars().count()).max().unwrap_or(0))
        .collect();
    let mut lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let cells: Vec<String> = row.iter().zip(&widths).map(|(cell, width)| pad_right(cell, *width)).collect();
            cells.join(" ; ").trim_end().to_owned()
        })
        .collect();
    lines.insert(0, title.to_owned());
    lines.join("\n")
}
