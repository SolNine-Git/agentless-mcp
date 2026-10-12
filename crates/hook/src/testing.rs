use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::EnvConfig;
use io::testing::Tree;

use crate::call::Clock;

// The wall time every test clock reports, and the generator writes into each expected record.
pub(crate) const WALL_SECONDS: u64 = 1_760_000_000;

// The crate directory, whose fixtures/ io::testing::cases reads.
pub(crate) const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

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
        let path = std::env::var_os("PATH").expect("the tests find git through PATH");
        let home = self.path().join("home");
        let fixed = [
            ("PATH", path),
            ("HOME", home.clone().into_os_string()),
            ("XDG_CACHE_HOME", home.join(".cache").into_os_string()),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
        ];
        let vars = fixed
            .into_iter()
            .chain(extra.iter().cloned())
            .map(|(name, value)| (OsString::from(name), value));
        EnvConfig::new(vars, self.path().to_path_buf())
    }

    // Runs one setup git command in the repository with a pinned identity and clock.
    pub(crate) fn git(&self, args: &[&str]) {
        let pinned = [
            ("GIT_AUTHOR_NAME", "hook tests"),
            ("GIT_AUTHOR_EMAIL", "hook@example.invalid"),
            ("GIT_AUTHOR_DATE", "2026-02-01T00:00:00Z"),
            ("GIT_COMMITTER_NAME", "hook tests"),
            ("GIT_COMMITTER_EMAIL", "hook@example.invalid"),
            ("GIT_COMMITTER_DATE", "2026-02-01T00:00:00Z"),
        ];
        let mut env = self.env(&[]).vars().to_vec();
        env.extend(pinned.map(|(name, value)| (OsString::from(name), OsString::from(value))));
        let mut command = vec![OsString::from("-C"), self.root().into_os_string()];
        command.extend(args.iter().map(OsString::from));
        let spec = io::Spec {
            program: OsString::from("git"),
            args: command,
            cwd: PathBuf::from("."),
            env,
            timeout: Duration::from_secs(30),
            stdout_cap: 1 << 20,
            stderr_cap: 1 << 16,
        };
        match io::run(&spec) {
            io::Outcome::Exited { status, .. } if status.success() => {}
            outcome => panic!("git {args:?}: {outcome:?}"),
        }
    }
}
