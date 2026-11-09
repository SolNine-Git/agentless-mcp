use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

pub fn pack_archive(paths: &[PathBuf], target: &Path) -> io::Result<u64> {
    let mut bundle = ZipWriter::new(File::create(target)?);
    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        bundle.start_file(name, Deflated)?;
        bundle.write_all(&fs::read(path)?)?;
    }
    bundle.finish()?;
    Ok(fs::metadata(target)?.len())
}
