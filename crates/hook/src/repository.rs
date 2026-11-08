//! The repository that holds a directory: its git top level, resolved.

use std::path::{Path, PathBuf};
use std::time::Duration;

use app::Services;
use git::Toplevel;

// The erosion gate's git timeout.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn root_of(services: &Services, dir: &Path) -> Option<PathBuf> {
    let Toplevel::Found(stdout) = services.git.toplevel(dir, GIT_TIMEOUT) else {
        return None;
    };
    let shown = String::from_utf8(stdout).ok()?;
    io::resolve(Path::new(text::strip(&shown))).ok()
}
