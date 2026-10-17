use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Result as IoResult, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use io::testing::{Tree, cases, latin};
use kernel::testing::PinnedClock;
use serde::Deserialize;
use serde_json::Value;

use super::Gate;
use super::check::check_with;
use super::mark::{mark_with, unlock_reason};
use crate::testing::MANIFEST_DIR;

const PLACEHOLDER: &str = "<SANDBOX>";
const PID_MASK: &str = "<PID>";
const MARKERS: &str = "markers";
const PRIVATE_MODE: u32 = 0o700;

// One reference case: setup operations, then hook runs in order in one directory.
#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    needs_modes: bool,
    setup: Vec<Vec<Value>>,
    steps: Vec<Step>,
}

// One hook run. cwd and log are below the directory; a null cwd is a removed one, a null log
// leaves the variable unset, and an empty log sets it empty.
#[derive(Debug, Deserialize)]
struct Step {
    hook: String,
    stdin: String,
    cwd: Option<String>,
    log: Option<String>,
    stderr_fails: bool,
    code: u8,
    stderr: String,
    log_text: Option<String>,
    markers: Option<Vec<(String, Option<String>)>>,
}

// What a run leaves: the exit code, stderr, the log file, and the marker directory's entries.
#[derive(Debug, PartialEq)]
struct Seen {
    code: u8,
    stderr: String,
    log_text: Option<String>,
    markers: Option<Vec<(String, Option<String>)>>,
}

impl Step {
    fn expected(&self) -> Seen {
        Seen {
            code: self.code,
            stderr: self.stderr.clone(),
            log_text: self.log_text.clone(),
            markers: self.markers.clone(),
        }
    }
}

// A stderr whose writes fail the way a closed pipe fails.
struct BrokenPipe;

impl Write for BrokenPipe {
    fn write(&mut self, _bytes: &[u8]) -> IoResult<usize> {
        Err(ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

struct Sandbox {
    tree: Tree,
    pid: String,
}

impl Sandbox {
    fn new() -> Self {
        let tree = Tree::new().unwrap_or_else(|error| panic!("a temporary tree: {error}"));
        let shown = tree.path().to_str().expect("a UTF-8 temporary directory");
        assert!(
            shown
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte)),
            "{shown} needs JSON escapes: set TMPDIR to a plain path"
        );
        Self {
            tree,
            pid: std::process::id().to_string(),
        }
    }

    fn shown(&self) -> &str {
        self.tree
            .path()
            .to_str()
            .expect("a UTF-8 temporary directory")
    }

    fn placed(&self, text: &str) -> String {
        text.replace(PLACEHOLDER, self.shown())
            .replace(PID_MASK, &self.pid)
    }

    fn masked(&self, text: &str) -> String {
        let probe = format!(".write-probe-{}", self.pid);
        text.replace(self.shown(), PLACEHOLDER)
            .replace(&probe, &format!(".write-probe-{PID_MASK}"))
    }

    // The path that the latin-1 text of its bytes names below the directory.
    fn at(&self, relative: &str) -> PathBuf {
        PathBuf::from(OsStr::from_bytes(&latin(&self.placed(relative))))
    }

    fn apply(&self, op: &[Value]) {
        let text = |at: usize| op[at].as_str().expect("a text operand");
        let relative = self.at(text(1));
        let done = match text(0) {
            "dir" => self.tree.create_dir(&relative).map(drop),
            "file" | "marker" => self.file(&relative, &latin(text(2))),
            "link" => self.tree.symlink(&relative, text(2)).map(drop),
            "mode" => {
                let mode = op[2].as_u64().expect("a mode");
                let mode = u32::try_from(mode).expect("a mode fits 32 bits");
                self.tree.set_mode(&relative, mode)
            }
            other => panic!("an unknown setup operation {other}"),
        };
        done.unwrap_or_else(|error| panic!("{op:?}: {error}"));
    }

    fn file(&self, relative: &Path, bytes: &[u8]) -> IoResult<()> {
        if let Some(parent) = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.tree.create_dir(parent)?;
        }
        self.tree.write_file(relative, bytes).map(drop)
    }

    // Every mode a case set goes back to the owner's, so the tree can be removed.
    fn restore(&self, setup: &[Vec<Value>]) {
        for op in setup.iter().filter(|op| op[0] == "mode") {
            let relative = self.at(op[1].as_str().expect("a text operand"));
            drop(self.tree.set_mode(&relative, PRIVATE_MODE));
        }
    }

    fn run(&self, step: &Step, clock: &PinnedClock) -> Seen {
        let cwd = step.cwd.as_ref().map(|relative| self.tree.join(relative));
        let log = step
            .log
            .as_ref()
            .map(|relative| self.log_variable(relative));
        let gate = Gate {
            markers: self.tree.join(MARKERS),
            log: log.as_deref(),
            cwd: cwd.as_deref(),
            clock,
            pid: std::process::id(),
        };
        let stdin = latin(&self.placed(&step.stdin));
        let mut stderr = Vec::new();
        let code = match (step.hook.as_str(), step.stderr_fails) {
            ("check", true) => check_with(&gate, &mut stdin.as_slice(), &mut BrokenPipe),
            ("check", false) => check_with(&gate, &mut stdin.as_slice(), &mut stderr),
            ("mark", _) => {
                mark_with(&gate, &mut stdin.as_slice());
                0
            }
            (other, _) => panic!("an unknown hook {other}"),
        };
        Seen {
            code,
            stderr: String::from_utf8(stderr).expect("UTF-8 stderr"),
            log_text: self.log_text(step),
            markers: self.markers(),
        }
    }

    fn log_variable(&self, relative: &str) -> OsString {
        if relative.is_empty() {
            return OsString::new();
        }
        self.tree.join(relative).into_os_string()
    }

    fn log_text(&self, step: &Step) -> Option<String> {
        let relative = step
            .log
            .as_deref()
            .filter(|relative| !relative.is_empty())?;
        let path = self.tree.join(relative);
        let bytes = path
            .is_file()
            .then(|| std::fs::read(&path).expect("a readable log"))?;
        Some(self.masked(&shown(&bytes)))
    }

    // The entries by their byte order, each with its bytes, or None for an entry that is not a
    // readable file. None for a marker directory that cannot be listed.
    fn markers(&self) -> Option<Vec<(String, Option<String>)>> {
        let entries = std::fs::read_dir(self.tree.join(MARKERS)).ok()?;
        let mut paths: Vec<PathBuf> = entries
            .map(|entry| entry.expect("an entry").path())
            .collect();
        paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        let listed = paths.iter().map(|path| {
            let name = path.file_name().expect("an entry name").as_bytes();
            let content = std::fs::read(path).ok().map(|bytes| shown(&bytes));
            (self.masked(&shown(name)), content)
        });
        Some(listed.collect())
    }
}

// The latin-1 text of bytes, as the fixtures store them.
fn shown(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

fn reference_clock() -> PinnedClock {
    let pinned: Vec<u64> = cases(MANIFEST_DIR, "gate/clock", "wall_ns");
    PinnedClock::at(UNIX_EPOCH + Duration::from_nanos(pinned[0]))
}

// A setup path, as the latin-1 text of its bytes, that is not UTF-8. A macOS file system refuses
// such a name (EILSEQ), so a case that creates one cannot run there.
fn needs_a_non_utf8_name(case: &Case) -> bool {
    let not_utf8 = |relative: &str| std::str::from_utf8(&latin(relative)).is_err();
    case.setup
        .iter()
        .any(|op| op[1].as_str().is_some_and(not_utf8))
}

// Each step's code, stderr, log and markers equal the reference's. The section holds
// `non_utf8_cases` cases that need a name that is not UTF-8, and macOS skips exactly those.
fn every_case_matches(section: &str, non_utf8_cases: usize) {
    let all: Vec<Case> = cases(MANIFEST_DIR, &format!("gate/{section}"), "cases");
    let needing = all
        .iter()
        .filter(|case| needs_a_non_utf8_name(case))
        .count();
    assert_eq!(
        needing, non_utf8_cases,
        "{section}: cases with a non-UTF-8 name"
    );
    let clock = reference_clock();
    for case in &all {
        if cfg!(target_os = "macos") && needs_a_non_utf8_name(case) {
            continue;
        }
        case_matches(section, case, &clock);
    }
}

// A case that needs file modes to refuse is skipped for root, which they do not refuse.
fn case_matches(section: &str, case: &Case, clock: &PinnedClock) {
    let sandbox = Sandbox::new();
    if case.needs_modes && !sandbox.tree.modes_bind() {
        return;
    }
    for op in &case.setup {
        sandbox.apply(op);
    }
    let seen: Vec<Seen> = case
        .steps
        .iter()
        .map(|step| sandbox.run(step, clock))
        .collect();
    sandbox.restore(&case.setup);
    for (index, (step, seen)) in case.steps.iter().zip(&seen).enumerate() {
        assert_eq!(
            *seen,
            step.expected(),
            "{section}: {}, step {index}",
            case.name
        );
    }
}

#[test]
fn the_reference_test_cases_match() {
    every_case_matches("tests", 0);
}

#[test]
fn the_shell_and_path_shapes_match() {
    every_case_matches("shapes", 10);
}

#[test]
fn the_log_lines_and_markers_match() {
    every_case_matches("records", 0);
}

#[test]
fn every_suppressed_failure_allows_as_the_reference_does() {
    every_case_matches("failures", 0);
}

#[test]
fn the_mark_cases_match() {
    every_case_matches("mark", 0);
}

#[test]
fn a_marker_that_the_reference_wrote_unlocks_the_gate() {
    every_case_matches("cross", 0);
}

// A step that these tests type: their outcomes deviate from the reference's on purpose.
fn step(hook: &str, payload: &str) -> Step {
    Step {
        hook: hook.to_owned(),
        stdin: payload.to_owned(),
        cwd: Some(String::new()),
        log: Some("gate.jsonl".to_owned()),
        stderr_fails: false,
        code: 0,
        stderr: String::new(),
        log_text: None,
        markers: None,
    }
}

const GLOB: &str = r#"{"session_id": "h", "tool_name": "Glob", "tool_input": {"pattern": "*"}}"#;
const ORIENT: &str = concat!(
    r#"{"session_id": "h", "tool_name": "mcp__agentless__orient", "#,
    r#""tool_input": {"operation": "map"}}"#,
);
const UNAVAILABLE: &str =
    r#""event": "native_search_allowed", "reason": "marker_storage_unavailable"}"#;

fn marker_name() -> String {
    format!("{}.json", crate::text::sha256_hex(b"h"))
}

fn read(sandbox: &Sandbox, relative: &str) -> Option<Vec<u8>> {
    std::fs::read(sandbox.tree.join(relative)).ok()
}

// A marker directory that is a symlink gives no marker and takes no probe and no marker, even
// when it leads to a directory of this user.
#[test]
fn a_symlinked_marker_directory_is_storage_unavailable() {
    let sandbox = Sandbox::new();
    let planted = format!("real/{}", marker_name());
    sandbox.apply(&[
        Value::from("file"),
        Value::from(planted.as_str()),
        Value::from("{}"),
    ]);
    let real = sandbox.tree.join("real");
    sandbox
        .tree
        .symlink(MARKERS, &real)
        .expect("a linked marker directory");
    let clock = reference_clock();

    let checked = sandbox.run(&step("check", GLOB), &clock);
    let after_mark = sandbox.run(&step("mark", ORIENT), &clock);

    assert_eq!(checked.code, 0);
    let log = checked.log_text.expect("a log line");
    assert!(log.ends_with(&format!("{UNAVAILABLE}\n")), "{log}");
    assert_eq!(after_mark.log_text, Some(log), "the mark writes no line");
    assert_eq!(read(&sandbox, &planted), Some(b"{}".to_vec()));
    let names = std::fs::read_dir(&real)
        .expect("the real directory")
        .count();
    assert_eq!(
        names, 1,
        "no probe and no new marker in the linked directory"
    );
}

// The probe is a new file: a symlink at its name, to a file or to nothing, is never followed.
#[test]
fn a_symlink_at_the_probe_name_is_not_followed() {
    let probe = format!("{MARKERS}/.write-probe-{}", std::process::id());
    for target in ["victim", "created"] {
        let sandbox = Sandbox::new();
        sandbox
            .tree
            .create_dir(MARKERS)
            .expect("a marker directory");
        sandbox
            .tree
            .write_file("victim", "keep")
            .expect("a victim file");
        let linked = sandbox.tree.join(target);
        sandbox
            .tree
            .symlink(&probe, &linked)
            .expect("a planted link");

        let checked = sandbox.run(&step("check", GLOB), &reference_clock());

        assert_eq!(checked.code, 0, "{target}");
        let log = checked.log_text.expect("a log line");
        assert!(log.ends_with(&format!("{UNAVAILABLE}\n")), "{log}");
        assert_eq!(read(&sandbox, "victim"), Some(b"keep".to_vec()));
        assert_eq!(read(&sandbox, "created"), None);
    }
}

// A probe file that a run left behind also fails the exclusive create.
#[test]
fn an_existing_probe_file_is_storage_unavailable() {
    let sandbox = Sandbox::new();
    let probe = format!("{MARKERS}/.write-probe-{}", std::process::id());
    sandbox.apply(&[
        Value::from("file"),
        Value::from(probe.as_str()),
        Value::from(""),
    ]);

    let checked = sandbox.run(&step("check", GLOB), &reference_clock());

    assert_eq!(checked.code, 0);
    let log = checked.log_text.expect("a log line");
    assert!(log.ends_with(&format!("{UNAVAILABLE}\n")), "{log}");
}

// The marker replaces a symlink at its name and leaves the link's target alone. It is private.
#[test]
fn a_symlink_at_the_marker_name_is_replaced_not_followed() {
    let sandbox = Sandbox::new();
    let planted = format!("{MARKERS}/{}", marker_name());
    sandbox
        .tree
        .create_dir(MARKERS)
        .expect("a marker directory");
    sandbox
        .tree
        .write_file("victim", "keep")
        .expect("a victim file");
    let victim = sandbox.tree.join("victim");
    sandbox
        .tree
        .symlink(&planted, &victim)
        .expect("a planted link");

    let after_mark = sandbox.run(&step("mark", ORIENT), &reference_clock());

    assert_eq!(after_mark.code, 0);
    assert_eq!(read(&sandbox, "victim"), Some(b"keep".to_vec()));
    let receipt = r#"{"version": 2, "reason": "orient.map", "tool": "mcp__agentless__orient"}"#;
    assert_eq!(read(&sandbox, &planted), Some(receipt.as_bytes().to_vec()));
    let metadata = std::fs::symlink_metadata(sandbox.tree.join(&planted)).expect("the marker");
    assert!(
        metadata.file_type().is_file(),
        "a regular file, not the link"
    );
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
}

// The surface fixture holds the reference hook's unlock reason for every v2 tool and operation.
#[test]
fn the_unlock_rule_matches_the_reference_for_every_tool_and_operation() {
    let app = format!("{MANIFEST_DIR}/../app");
    let rows: Vec<(String, Option<String>, Option<String>)> =
        cases(&app, "surface/surface", "unlock");
    for (tool, operation, reason) in rows {
        let name = format!("mcp__agentless__{tool}");
        let found = unlock_reason(Some(&name), operation.as_deref());
        assert_eq!(found, reason, "{tool} {operation:?}");
    }
}
