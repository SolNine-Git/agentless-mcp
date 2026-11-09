use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::archive::pack_archive;

pub fn backup_all(paths: &[PathBuf], target: &Path) -> io::Result<u64> {
    let size = pack_archive(paths, target)?;
    let mut bundle = ZipWriter::new(File::create(target.with_extension("sum"))?);
    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        bundle.start_file(name, Deflated)?;
        bundle.write_all(&fs::read(path)?)?;
    }
    bundle.finish()?;
    Ok(size.max(fs::metadata(target)?.len()))
}
