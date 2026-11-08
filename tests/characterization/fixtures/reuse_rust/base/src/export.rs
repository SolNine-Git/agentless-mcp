use std::fs::OpenOptions;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

const HEADER: &str = "name,count\n";

pub fn export_csv(rows: &[Vec<String>], path: &Path) -> io::Result<usize> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(HEADER.as_bytes())?;
    file.write_all(rows.concat().join(",").as_bytes())?;
    file.sync_data()?;
    Ok(rows.len())
}
