use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::EnvConfig;
use app::hooks::Hook;
use io::testing::{Identity, Tree};
use kernel::Clock;

use crate::run::{Facts, Outcome, Run};

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

const INVOICE_TEXT: &str = "class InvoiceTotal:\n    def rounded(self):\n        return \
                            round_cents(self.amount)\n\n\ndef round_cents(value):\n    return \
                            round(value, 2)\n";
const REPORT_TEXT: &str = "from src.billing.invoice import InvoiceTotal\n\n\n\
                           def monthly_report(items):\n    return [InvoiceTotal() for _ in items]\n";

// A sandbox with a committed two-file Python repository and a TOML file, the services, the
// ledger and the gate log of the runs of the trace surface's hooks.
pub(crate) struct Rig {
    pub(crate) sandbox: Sandbox,
    pub(crate) services: app::Services,
    pub(crate) log: OsString,
}

impl Rig {
    pub(crate) const INVOICE: &'static str = "src/billing/invoice.py";
    pub(crate) const REPORT: &'static str = "src/billing/report.py";
    pub(crate) const SETTINGS: &'static str = "config/settings.toml";
    const LOG: &'static str = "gate.jsonl";

    pub(crate) fn new() -> Self {
        let sandbox = Sandbox::new();
        sandbox.mkdir("repo");
        sandbox.git(&["init", "--quiet", "-b", "main"]);
        drop(sandbox.put(&format!("repo/{}", Self::INVOICE), INVOICE_TEXT));
        drop(sandbox.put(&format!("repo/{}", Self::REPORT), REPORT_TEXT));
        drop(sandbox.put(
            &format!("repo/{}", Self::SETTINGS),
            "[billing]\nround = 2\n",
        ));
        sandbox.git(&["add", "-A"]);
        sandbox.git(&["commit", "--quiet", "-m", "fixture"]);
        // No repository above the sandbox can answer for a directory outside the fixture.
        let ceiling = sandbox.path().as_os_str().to_owned();
        let env = sandbox.env(&[("GIT_CEILING_DIRECTORIES", ceiling)]);
        let services = app::Services::new(&env);
        let log = sandbox.path().join(Self::LOG).into_os_string();
        Self {
            sandbox,
            services,
            log,
        }
    }

    pub(crate) fn ledger(&self) -> PathBuf {
        self.sandbox.path().join("ledger")
    }

    // The absolute path of a file below the repository.
    pub(crate) fn file(&self, relative: &str) -> PathBuf {
        self.sandbox.root().join(relative)
    }

    // A run of `hook` in the sandbox, with a deadline that no test job reaches.
    pub(crate) fn run<'a>(&'a self, hook: Hook, clock: &'a dyn Clock) -> Run<'a> {
        Run {
            hook,
            ledger: self.ledger(),
            log: Some(self.log.as_os_str()),
            cwd: None,
            clock,
            deadline: Duration::from_secs(30),
        }
    }

    // The stdout of one run whose decision is `decide`.
    pub(crate) fn answer(
        run: &Run<'_>,
        decide: impl FnOnce(&mut Facts) -> Result<Outcome, &'static str>,
    ) -> String {
        let mut stdout = Vec::new();
        crate::run::answered(run, &mut stdout, decide);
        String::from_utf8(stdout).expect("UTF-8 stdout")
    }

    // The read-card reply to a Read of `relative` in the context.
    pub(crate) fn read_reply(&self, session: &str, agent: Option<&str>, relative: &str) -> String {
        let clock = Steps::fixed();
        let run = self.run(Hook::ReadCard, &clock);
        let mut payload = serde_json::json!({
            "session_id": session,
            "tool_name": "Read",
            "tool_input": {"file_path": self.file(relative)},
        });
        if let Some(agent) = agent {
            payload["agent_id"] = serde_json::json!(agent);
        }
        let stdin = payload.to_string();
        Self::answer(&run, |facts| {
            let card = crate::read_card::in_process;
            crate::read_card::decide(&run, card, &self.services, &mut stdin.as_bytes(), facts)
        })
    }

    pub(crate) fn lines(&self) -> Vec<serde_json::Value> {
        let path = self.sandbox.path().join(Self::LOG);
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    pub(crate) fn reasons(&self) -> Vec<String> {
        let lines = self.lines();
        let reasons = lines
            .iter()
            .map(|line| line["reason"].as_str().unwrap_or("?"));
        reasons.map(str::to_owned).collect()
    }
}

// The context string of a hook's reply, after a check of the reply's shape.
pub(crate) fn context_of(stdout: &str, event: &str) -> String {
    let reply: serde_json::Value = serde_json::from_str(stdout).expect("one JSON object");
    assert_eq!(
        reply.as_object().map(serde_json::Map::len),
        Some(1),
        "{stdout}"
    );
    let output = &reply["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], event, "{stdout}");
    output["additionalContext"]
        .as_str()
        .expect("a context string")
        .to_owned()
}
