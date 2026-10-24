//! The background index: one refresh of a repository's agentless.db for each generation, on its
//! own thread, which the first call that reads the repository at that generation starts.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use app::{IndexReport, IndexRequest, Noted, Repo, Services, Session};
use store::CacheState;

use crate::offload::{Job, Offload, Threads};
use crate::server::{Arming, Log};

// The generation that an attempt records when it could not read one; no generation equals it, so
// the repository is not tried again until the process restarts.
pub(crate) const UNAVAILABLE: &str = "unavailable";

// The index run: `app::index`, which the tests replace.
pub(crate) type Index =
    dyn Fn(&Services, &Repo, &git::Snapshot, &IndexRequest) -> Indexed + Send + Sync;
pub(crate) type Indexed = Noted<Result<IndexReport, app::Error>>;

// One registry of attempts for the server: one attempt per database and generation.
pub(crate) struct AutoIndex {
    services: Arc<Services>,
    request: IndexRequest,
    log: Arc<dyn Log>,
    offload: Arc<dyn Offload>,
    index: Arc<Index>,
    runs: Mutex<HashMap<PathBuf, Run>>,
}

// One database's last attempt: the generation it targets and, when its thread started, whether
// that thread still runs.
struct Run {
    live: Option<Arc<AtomicBool>>,
    generation: String,
}

impl Run {
    fn running(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|live| live.load(Ordering::SeqCst))
    }
}

// Owned by the refresh's job: its drop ends the run, after a panic too.
struct Running(Arc<AtomicBool>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// What one arming does.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Attempt {
    Nothing,
    // The generation to record without a thread, and the warning to log.
    Record(String, String),
    Start(String),
}

impl AutoIndex {
    pub(crate) fn new(
        services: Arc<Services>,
        max_cache_bytes: Option<OsString>,
        log: Arc<dyn Log>,
    ) -> AutoIndex {
        let offload = Arc::new(Threads("tag-auto-index"));
        AutoIndex::with(
            services,
            max_cache_bytes,
            log,
            offload,
            Arc::new(app::index),
        )
    }

    pub(crate) fn with(
        services: Arc<Services>,
        max_cache_bytes: Option<OsString>,
        log: Arc<dyn Log>,
        offload: Arc<dyn Offload>,
        index: Arc<Index>,
    ) -> AutoIndex {
        AutoIndex {
            services,
            request: IndexRequest {
                force: false,
                max_cache_bytes,
            },
            log,
            offload,
            index,
            runs: Mutex::new(HashMap::new()),
        }
    }

    fn runs(&self) -> MutexGuard<'_, HashMap<PathBuf, Run>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // The generation of the repository now. A usable database read it when the session opened.
    fn generation(&self, session: &Session) -> Result<String, String> {
        if let CacheState::Indexed {
            repo_generation, ..
        } = &session.cache.status.state
        {
            return Ok(repo_generation.clone());
        }
        let tree = session.snapshot.tree_oid.as_deref();
        let outcome = app::repo_generation(&self.services, &session.repo.root, tree);
        for line in &outcome.diagnostics {
            self.log.line(line);
        }
        outcome.value.map_err(|error| error.to_string())
    }

    // Under the lock: a racer that registered this generation already started its run.
    fn start(&self, session: &Session, generation: String) -> Result<(), String> {
        let database = &session.cache.status.path;
        let mut runs = self.runs();
        if runs
            .get(database)
            .is_some_and(|run| run.generation == generation)
        {
            return Ok(());
        }
        let live = Arc::new(AtomicBool::new(true));
        let started = self
            .offload
            .start(self.job(session, Running(Arc::clone(&live))));
        let run = Run {
            live: started.is_ok().then_some(live),
            generation,
        };
        runs.insert(database.clone(), run);
        started.map_err(|error| format!("the background index thread did not start: {error}"))
    }

    fn job(&self, session: &Session, running: Running) -> Job {
        let services = Arc::clone(&self.services);
        let index = Arc::clone(&self.index);
        let log = Arc::clone(&self.log);
        let (repo, snapshot) = (session.repo.clone(), session.snapshot.clone());
        let request = self.request.clone();
        Box::new(move || {
            let _running = running;
            let outcome = index(&services, &repo, &snapshot, &request);
            for line in refresh_lines(&repo.root, outcome) {
                log.line(&line);
            }
        })
    }

    // An attempt that stopped before a thread started. It never replaces a live run.
    pub(crate) fn record(&self, database: &Path, generation: String) {
        let mut runs = self.runs();
        if runs.get(database).is_some_and(Run::running) {
            return;
        }
        let run = Run {
            live: None,
            generation,
        };
        runs.insert(database.to_path_buf(), run);
    }

    // What the registry holds for `database` before an arming decides.
    fn last(&self, database: &Path) -> Last {
        match self.runs().get(database) {
            Some(run) if run.running() || run.generation == UNAVAILABLE => Last::Blocking,
            run => Last::Attempted(run.map(|run| run.generation.clone())),
        }
    }
}

// A live run, or an attempt that could not read the generation, blocks an arming; otherwise the
// generation of the last attempt, if any, is what the arming compares.
enum Last {
    Blocking,
    Attempted(Option<String>),
}

impl Arming for AutoIndex {
    fn arm(&self, session: &Session) -> Result<(), String> {
        let database = &session.cache.status.path;
        let Last::Attempted(previous) = self.last(database) else {
            return Ok(());
        };
        let root = session.repo.root.display().to_string();
        let state = &session.cache.status.state;
        match attempt(previous.as_deref(), state, self.generation(session), &root) {
            Attempt::Nothing => Ok(()),
            Attempt::Record(generation, warning) => {
                self.record(database, generation);
                self.log.line(&warning);
                Ok(())
            }
            Attempt::Start(generation) => self.start(session, generation),
        }
    }

    fn refreshing(&self, database: &Path) -> bool {
        self.runs().get(database).is_some_and(Run::running)
    }
}

// The arming of a server whose background index is off.
pub(crate) struct Disarmed;

impl Arming for Disarmed {
    fn arm(&self, _session: &Session) -> Result<(), String> {
        Ok(())
    }

    fn refreshing(&self, _database: &Path) -> bool {
        false
    }
}

// What an arming does, from the last attempt's generation, the session's cache and the generation
// now. A database at the generation now needs nothing; an absent or refused one needs a refresh.
pub(crate) fn attempt(
    previous: Option<&str>,
    state: &CacheState,
    generation: Result<String, String>,
    root: &str,
) -> Attempt {
    let generation = match generation {
        Ok(generation) => generation,
        Err(error) => {
            let warning = format!("background index refresh for {root} skipped: {error}");
            return Attempt::Record(UNAVAILABLE.to_owned(), warning);
        }
    };
    let current = matches!(
        state,
        CacheState::Indexed { generation: built, .. } if *built == generation
    );
    if current || previous == Some(generation.as_str()) {
        return Attempt::Nothing;
    }
    Attempt::Start(generation)
}

// The log lines of one refresh: its warnings, then why it failed. A refresh that the lock of
// another index run stopped needs no line: that run's result serves this one.
pub(crate) fn refresh_lines(
    root: &Path,
    outcome: Noted<Result<IndexReport, app::Error>>,
) -> Vec<String> {
    let mut lines = outcome.diagnostics;
    match outcome.value {
        Ok(_) | Err(app::Error::Kind(kernel::Error::CacheLocked(_))) => {}
        Err(error) => {
            let root = root.display();
            lines.push(format!(
                "background index refresh for {root} failed: {error}"
            ));
        }
    }
    lines
}
