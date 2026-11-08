use std::fs::OpenOptions;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

const COLUMNS: &str = "name\tcount\n";

pub fn dump_tsv(rows: &[Vec<String>], path: &Path) -> io::Result<usize> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(COLUMNS.as_bytes())?;
    file.write_all(rows.concat().join("\t").as_bytes())?;
    file.flush()?;
    file.sync_data()?;
    Ok(rows.len())
}
