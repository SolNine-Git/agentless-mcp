use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::EnvConfig;
use io::testing::{Identity, Tree};

use crate::call::Clock;

// The wall time every test clock reports, and the generator writes into each expected record.
pub(crate) const WALL_SECONDS: u64 = 1_760_000_000;

// The crate directory, whose fixtures/ io::testing::cases reads.
pub(crate) const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

// The commit identity of every setup repository. The hook generator pins the same one.
const IDENTITY: Identity = Identity {
    name: "hook tests",
    email: "hook@example.invalid",
    date: "2026-02-01T00:00:00Z",
};

pub(crate) use io::testing::latin;

// Monotonic time steps through `offsets` from a fixed start, one per call, and the last offset
// repeats. The wall time is fixed.
pub(crate) struct Steps {
    base: Instant,
    offsets: Vec<Duration>,
    calls: AtomicUsize,
}

impl Steps {
    pub(crate) fn new(offsets: &[Duration]) -> Self {
        Self {
            base: Instant::now(),
            offsets: offsets.to_vec(),
            calls: AtomicUsize::new(0),
        }
    }

    pub(crate) fn fixed() -> Self {
        Self::new(&[Duration::ZERO])
    }
}

impl Clock for Steps {
    fn now(&self) -> Instant {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        let last = self.offsets.len() - 1;
        self.base + self.offsets[call.min(last)]
    }

    fn wall(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(WALL_SECONDS)
    }
}

// A test's own directory, with an environment in which no git or cache config reaches the code.
pub(crate) struct Sandbox {
    tree: Tree,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
        let tree = Tree::new().unwrap_or_else(|error| panic!("a temporary tree: {error}"));
        Self { tree }
    }

    pub(crate) fn path(&self) -> &Path {
        self.tree.path()
    }

    pub(crate) fn root(&self) -> PathBuf {
        self.path().join("repo")
    }

    // Writes `bytes` to `relative`, with its missing parent directories.
    pub(crate) fn put(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        if let Some((parent, _)) = relative.rsplit_once('/') {
            self.mkdir(parent);
        }
        let written = self.tree.write_file(relative, bytes);
        written.unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    pub(crate) fn mkdir(&self, relative: &str) -> PathBuf {
        self.tree.mkdir(relative)
    }

    pub(crate) fn symlink(&self, relative: &str, target: &str) {
        let target = self.placed(target);
        let linked = self.tree.symlink(relative, target);
        linked.unwrap_or_else(|error| panic!("{relative}: {error}"));
    }

    pub(crate) fn set_mode(&self, relative: &str, mode: u32) {
        self.tree.chmod(relative, mode);
    }

    // False for root, which searches a directory whose mode is 000.
    pub(crate) fn modes_bind(&self) -> bool {
        self.tree.modes_bind()
    }

    // The fixture text with the sandbox's own paths in place of the placeholders.
    pub(crate) fn placed(&self, text: &str) -> String {
        text.replace("<ROOT>", &self.root().display().to_string())
            .replace("<SANDBOX>", &self.path().display().to_string())
    }

    // The sandbox's own paths turned back into the placeholders.
    pub(crate) fn placeholders(&self, text: &str) -> String {
        text.replace(&self.root().display().to_string(), "<ROOT>")
            .replace(&self.path().display().to_string(), "<SANDBOX>")
    }

    // The start-up environment: PATH to find git, HOME and the cache home inside the sandbox.
    pub(crate) fn env(&self, extra: &[(&str, OsString)]) -> EnvConfig {
        let mut vars = self.vars();
        vars.extend(
            extra
                .iter()
                .map(|(name, value)| (OsString::from(name), value.clone())),
        );
        EnvConfig::new(vars, self.path().to_path_buf())
    }

    fn vars(&self) -> Vec<(OsString, OsString)> {
        io::testing::hermetic_env(&self.path().join("home"))
    }

    // Runs one setup git command in the repository with a pinned identity and clock.
    pub(crate) fn git(&self, args: &[&str]) {
        drop(io::testing::run_git(
            &self.root(),
            args,
            &self.vars(),
            &IDENTITY,
        ));
    }
}
