use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use app::{IndexReport, IndexRequest, Noted, Repo, Services, Session};
use prompts::messages::CACHE_ABSENT_REFRESHING;
use serde_json::json;
use store::{CacheState, Refusal};

use super::{Kept, Sandbox, Setup};
use crate::autoindex::{Attempt, AutoIndex, Index, UNAVAILABLE, attempt, refresh_lines};
use crate::dispatch::UNPLANNED;
use crate::offload::{Job, Offload};
use crate::{Arming, Ports};

const WAIT: Duration = Duration::from_secs(20);
const GENERATION: &str = "0123abcd";

// Each job on its own thread, kept so that the test can wait for its end.
#[derive(Default)]
struct Joined(Mutex<Vec<JoinHandle<()>>>);

impl Joined {
    fn join_all(&self) {
        let handles: Vec<JoinHandle<()>> = self.0.lock().expect("handles").drain(..).collect();
        for handle in handles {
            drop(handle.join());
        }
    }
}

impl Offload for Joined {
    fn start(&self, job: Job) -> std::io::Result<()> {
        let handle = std::thread::Builder::new().spawn(job)?;
        self.0.lock().expect("handles").push(handle);
        Ok(())
    }
}

struct Refused;

impl Offload for Refused {
    fn start(&self, _job: Job) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(11))
    }
}

// The index of the tests: each run reports its start, waits for a release, then ends with the
// scripted outcome.
struct Scripted {
    started: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
    outcome: fn() -> Noted<Result<IndexReport, app::Error>>,
}

fn scripted(
    outcome: fn() -> Noted<Result<IndexReport, app::Error>>,
) -> (Arc<Index>, Receiver<()>, Sender<()>) {
    let (started, on_start) = mpsc::channel();
    let (release, on_release) = mpsc::channel();
    let script = Scripted {
        started: Mutex::new(started),
        release: Mutex::new(on_release),
        outcome,
    };
    let index = move |_: &Services, _: &Repo, _: &git::Snapshot, _: &IndexRequest| {
        script.started.lock().expect("a sender").send(()).ok();
        let release = script.release.lock().expect("a receiver");
        release.recv_timeout(WAIT).ok();
        (script.outcome)()
    };
    (Arc::new(index), on_start, release)
}

fn locked() -> Noted<Result<IndexReport, app::Error>> {
    let error = kernel::Error::CacheLocked("another run holds the lock".to_owned());
    Noted::new(Err(error.into()))
}

fn broken() -> Noted<Result<IndexReport, app::Error>> {
    let error = kernel::Error::OperationFailed("the disk is full".to_owned());
    Noted {
        value: Err(error.into()),
        diagnostics: vec!["a warning of the walk".to_owned()],
    }
}

fn panicking() -> Noted<Result<IndexReport, app::Error>> {
    panic!("an injected panic on the index thread");
}

// A sandbox with one committed repository, and its session over agentless.db.
fn repository() -> (Sandbox, Arc<Services>) {
    let sandbox = Sandbox::new();
    sandbox.step(&["init", "repo"]);
    sandbox.step(&["write", "repo/a.py", "def a():\n    return 1\n"]);
    sandbox.step(&["commit", "repo"]);
    let services = sandbox.services();
    (sandbox, services)
}

fn session(sandbox: &Sandbox, services: &Services) -> Session {
    let root = io::resolve(&sandbox.root.join("repo")).expect("a root");
    let repo = Repo::load(services, &root).expect("a repository");
    let opening = Session::open_facts(services, repo, false);
    opening.session.value.expect("a session")
}

fn auto_index(
    services: &Arc<Services>,
    log: &Arc<Kept>,
    offload: Arc<dyn Offload>,
    index: Arc<Index>,
) -> AutoIndex {
    AutoIndex::with(
        Arc::clone(services),
        None,
        Arc::clone(log) as _,
        offload,
        index,
    )
}

#[test]
fn the_decision_follows_the_generation_and_the_last_attempt() {
    let absent = CacheState::Absent { note: None };
    let refused = CacheState::Absent {
        note: Some(Refusal::NoIndex),
    };
    let indexed = |built: &str| CacheState::Indexed {
        generation: built.to_owned(),
        repo_generation: GENERATION.to_owned(),
        repo_root: "/r".to_owned(),
    };
    let now = || Ok(GENERATION.to_owned());
    let start = Attempt::Start(GENERATION.to_owned());
    assert_eq!(attempt(None, &absent, now(), "/r"), start);
    assert_eq!(attempt(None, &refused, now(), "/r"), start);
    assert_eq!(attempt(None, &indexed("ffff0000"), now(), "/r"), start);
    assert_eq!(
        attempt(None, &indexed(GENERATION), now(), "/r"),
        Attempt::Nothing
    );
    assert_eq!(
        attempt(Some(GENERATION), &absent, now(), "/r"),
        Attempt::Nothing
    );
    let warning = "background index refresh for /r skipped: the walk failed".to_owned();
    let failed = attempt(None, &absent, Err("the walk failed".to_owned()), "/r");
    assert_eq!(failed, Attempt::Record(UNAVAILABLE.to_owned(), warning));
}

#[test]
fn a_refresh_logs_its_warnings_and_a_failure_but_not_a_held_lock() {
    let root = PathBuf::from("/r");
    let lines = refresh_lines(&root, broken());
    let failed = "background index refresh for /r failed: the disk is full";
    assert_eq!(lines, ["a warning of the walk", failed]);
    assert!(refresh_lines(&root, locked()).is_empty());
}

// One run for each database and generation: no second run while one runs, and none after it
// ended at the same generation.
#[test]
fn one_refresh_runs_for_each_generation() {
    let (sandbox, services) = repository();
    let log = Arc::new(Kept::default());
    let joined = Arc::new(Joined::default());
    let (index, on_start, release) = scripted(locked);
    let armed = auto_index(&services, &log, joined.clone(), index);
    let session = session(&sandbox, &services);
    let database = session.cache.status.path.clone();
    armed.arm(&session).expect("an armed refresh");
    on_start.recv_timeout(WAIT).expect("a started refresh");
    assert!(armed.refreshing(&database));
    armed.arm(&session).expect("no second refresh");
    release.send(()).expect("a waiting refresh");
    joined.join_all();
    assert!(!armed.refreshing(&database));
    armed
        .arm(&session)
        .expect("no refresh at the same generation");
    assert!(on_start.try_recv().is_err());
    assert!(log.lines().is_empty(), "{:?}", log.lines());
}

#[test]
fn a_new_generation_arms_the_refresh_again() {
    let (sandbox, services) = repository();
    let log = Arc::new(Kept::default());
    let joined = Arc::new(Joined::default());
    let (index, on_start, release) = scripted(broken);
    let armed = auto_index(&services, &log, joined.clone(), index);
    armed.arm(&session(&sandbox, &services)).expect("a refresh");
    on_start.recv_timeout(WAIT).expect("a started refresh");
    release.send(()).expect("a waiting refresh");
    joined.join_all();
    sandbox.step(&["write", "repo/b.py", "def b():\n    return 2\n"]);
    sandbox.step(&["commit", "repo"]);
    armed
        .arm(&session(&sandbox, &services))
        .expect("a second refresh");
    on_start
        .recv_timeout(WAIT)
        .expect("a second started refresh");
    release.send(()).expect("a waiting refresh");
    joined.join_all();
    let root = sandbox.root.join("repo").display().to_string();
    let failed = format!("background index refresh for {root} failed: the disk is full");
    assert_eq!(
        log.lines().iter().filter(|line| **line == failed).count(),
        2
    );
}

#[test]
fn an_unreadable_generation_is_never_tried_again() {
    let (sandbox, services) = repository();
    let log = Arc::new(Kept::default());
    let (index, on_start, _release) = scripted(locked);
    let armed = auto_index(&services, &log, Arc::new(Joined::default()), index);
    let session = session(&sandbox, &services);
    armed.record(&session.cache.status.path, UNAVAILABLE.to_owned());
    armed.arm(&session).expect("no refresh");
    assert!(on_start.try_recv().is_err());
}

// Failure injection: a panic on the refresh's thread ends the run, which is not tried again at
// that generation.
#[test]
fn a_panic_in_the_refresh_ends_its_run() {
    let (sandbox, services) = repository();
    let log = Arc::new(Kept::default());
    let joined = Arc::new(Joined::default());
    let (index, on_start, release) = scripted(panicking);
    let armed = auto_index(&services, &log, joined.clone(), index);
    let session = session(&sandbox, &services);
    armed.arm(&session).expect("a refresh");
    on_start.recv_timeout(WAIT).expect("a started refresh");
    release.send(()).expect("a waiting refresh");
    joined.join_all();
    assert!(!armed.refreshing(&session.cache.status.path));
    armed.arm(&session).expect("no second refresh");
    assert!(on_start.try_recv().is_err());
}

// Failure injection: a refused thread fails the call as an unplanned failure, as the reference's
// thread start does, and the attempt is recorded.
#[test]
fn a_refused_refresh_thread_fails_the_call_once() {
    let (sandbox, services) = repository();
    let log = Arc::new(Kept::default());
    let (index, on_start, _release) = scripted(locked);
    let armed = Arc::new(auto_index(&services, &log, Arc::new(Refused), index));
    let ports = Ports {
        arming: armed.clone(),
        log: log.clone(),
    };
    let setup = Setup {
        roots: &["repo"],
        files: &[],
        launch: None,
    };
    let server = sandbox.server_with(&setup, Arc::clone(&services), ports);
    let dir = json!({"operation": "dir"});
    let first = server.answer("read", dir.as_object().cloned());
    assert_eq!((first.text.as_str(), first.is_error), (UNPLANNED, true));
    let detail = "unhandled error in an agentless-mcp tool call: read: the background index thread \
                  did not start: Resource temporarily unavailable (os error 11)";
    assert_eq!(log.lines(), [detail]);
    let second = server.answer("read", dir.as_object().cloned());
    assert!(!second.is_error, "{}", second.text);
    assert!(on_start.try_recv().is_err());
}

// While the refresh runs, the answer's receipt names it in place of the remediation.
#[test]
fn the_receipt_names_a_running_refresh() {
    let (sandbox, services) = repository();
    let log = Arc::new(Kept::default());
    let joined = Arc::new(Joined::default());
    let (index, on_start, release) = scripted(locked);
    let armed = Arc::new(auto_index(&services, &log, joined.clone(), index));
    let ports = Ports {
        arming: armed.clone(),
        log: log.clone(),
    };
    let setup = Setup {
        roots: &["repo"],
        files: &[],
        launch: None,
    };
    let server = sandbox.server_with(&setup, Arc::clone(&services), ports);
    let dir = json!({"operation": "dir"});
    let answer = server.answer("read", dir.as_object().cloned());
    on_start.recv_timeout(WAIT).expect("a started refresh");
    let refreshing = format!("cache: none ({CACHE_ABSENT_REFRESHING})");
    assert!(answer.text.contains(&refreshing), "{}", answer.text);
    release.send(()).expect("a waiting refresh");
    joined.join_all();
    let later = server.answer("read", dir.as_object().cloned());
    assert!(
        !later.text.contains(CACHE_ABSENT_REFRESHING),
        "{}",
        later.text
    );
}
