//! The name index and the resolved graph that one call reads, each built at its first use, and the
//! facts and graphs that a server keeps between its calls. A kept graph borrows the scan it was
//! built from, so it lives on the stack of one thread per root, and a call sends that thread a job
//! that owns everything it reads. A server's call starts the build of its scan's graph on a helper
//! of that thread, so a later card finds it built.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use git::RepoFile;
use graph::{
    Aliases, AmbiguousDegrees, Cycle, FileFacts, KeptParts, NameIndex, Rebuild, Resolved, Resolver,
    Scopes, SymbolGraph, import_cycles,
};
use kernel::{Clock, Deadline, Stopped, checkpoint};

use super::retention::{Called, RETAINED, Updates};
use super::store::Scan;
use crate::detached::{answered_over, drop_detached};
use crate::parallel::{map_in_order, threads};
use crate::scan::{KeptFacts, RepoFacts, Skipped, rescan, scan_walked};
use crate::{Error, Noted, Reading, Services};

// How long a call waits for its root's graph thread between two checks of its deadline.
const WAIT: Duration = Duration::from_millis(10);
// The files that one scan thread compares at a time when a kept graph checks a call's scan.
const COMPARED_FILES: usize = 64;

// One call's scan: the files with facts, which a kept graph shares, the files passed over, and the
// warnings that each operation over the scan reports, as a scan of its own would give them.
pub(crate) struct CallScan {
    pub(crate) files: Arc<Vec<Arc<FileFacts>>>,
    pub(crate) skipped: Vec<Skipped>,
    pub(crate) warnings: Vec<String>,
}

// Scans the repository once and gives the scan to `answer`. The scan's warnings come first.
pub(crate) fn over_call_scan<T>(
    services: &Services,
    reading: &Reading<'_>,
    answer: impl FnOnce(&CallScan) -> Noted<Result<T, Error>>,
) -> Noted<Result<T, Error>> {
    let loaded = RepoFacts::load(services, reading);
    let warnings = loaded.diagnostics.clone();
    let scanned = loaded.map(|facts| {
        facts.map(|facts| CallScan {
            files: Arc::new(shared(facts.files)),
            skipped: facts.skipped,
            warnings,
        })
    });
    scanned.then_ok(|scan| answered_over(scan, answer))
}

// Scans the walked `files` once and gives the scan to `answer`. The walk's `warnings` come first,
// as the scan's own walk gives them.
pub(crate) fn over_walked_scan<T>(
    reading: &Reading<'_>,
    files: &[RepoFile],
    warnings: &[String],
    keeping: Keeping<'_>,
    answer: impl FnOnce(&CallScan) -> Noted<Result<T, Error>>,
) -> Noted<Result<T, Error>> {
    let (root, source, deadline) = (&reading.repo.root, reading.facts, reading.deadline);
    let scanned = match keeping {
        Keeping::Server(graphs, kept_root) => {
            let kept = graphs.facts(kept_root);
            let scanned = rescan(root, files, source, deadline, &kept);
            scanned.map(|scanned| {
                scanned.map(|(kept, skipped)| {
                    let files = Arc::clone(&kept.files);
                    graphs.keep(kept_root, kept);
                    graphs.prebuild(kept_root, &files);
                    (files, skipped)
                })
            })
        }
        Keeping::Own | Keeping::Thread(_) => {
            let scanned = scan_walked(root, files, source, deadline);
            let shared = |facts: RepoFacts| (Arc::new(shared(facts.files)), facts.skipped);
            scanned.map(|scanned| scanned.map(shared))
        }
    };
    let mut diagnostics = warnings.to_vec();
    diagnostics.extend(scanned.diagnostics);
    let warnings = diagnostics.clone();
    let value = scanned
        .value
        .map_err(Error::from)
        .map(|(files, skipped)| CallScan {
            files,
            skipped,
            warnings,
        });
    Noted { value, diagnostics }.then_ok(|scan| answered_over(scan, answer))
}

fn shared(files: Vec<FileFacts>) -> Vec<Arc<FileFacts>> {
    files.into_iter().map(Arc::new).collect()
}

// Who builds the graph that a call graph answers over.
#[derive(Clone, Copy)]
pub(crate) enum Keeping<'a> {
    // The call builds its own.
    Own,
    // The server keeps the graph of the root on the root's graph thread.
    Server(&'a KeptGraphs, &'a Path),
    // The graph thread builds it, and counts each build.
    Thread(&'a Counts),
}

// The name index, import scopes, resolver and resolved graph of one scan, whose facts live for
// `'s`. The first answer that needs one builds it, and every later answer reads the same one.
#[derive(Clone, Copy)]
pub(crate) struct CallGraph<'f, 's> {
    files: &'f Arc<Vec<Arc<FileFacts>>>,
    listed: &'s [&'s FileFacts],
    built: &'f Built<'s>,
    // The resolver borrows the index and the scopes, so it lives in the frame of one scan.
    resolver: &'f OnceLock<Resolver<'f, 's>>,
    deadline: Option<&'f Deadline>,
    keeping: Keeping<'f>,
}

// The parts of a call graph that borrow only its scan's facts. A lock holds each part, so that a
// graph thread and its prebuild share them and build each once.
#[derive(Default)]
struct Built<'s> {
    index: OnceLock<NameIndex<'s>>,
    scopes: OnceLock<Scopes<'s>>,
    aliases: OnceLock<Aliases<'s>>,
    graph: OnceLock<Result<SymbolGraph<'s>, Stopped>>,
    degrees: OnceLock<AmbiguousDegrees>,
    imports: OnceLock<SymbolGraph<'s>>,
    cycles: OnceLock<Vec<Cycle<'s>>>,
}

impl<'s> Built<'s> {
    // Carries each built part from the scan `before` to the scan `after` of the same paths,
    // resolving at most `sites` percent of the scan's sites again.
    fn updated(
        &mut self,
        before: &'s [&'s FileFacts],
        after: &'s [&'s FileFacts],
        sites: usize,
    ) -> Result<Resolved, Rebuild> {
        let graph = match self.graph.get_mut() {
            Some(Ok(graph)) => Some(graph),
            Some(Err(_)) => return Err(Rebuild::Unbuilt),
            None => None,
        };
        let parts = KeptParts {
            index: self.index.get_mut(),
            scopes: self.scopes.get_mut(),
            graph,
            degrees: self.degrees.get_mut(),
            aliases: self.aliases.get_mut(),
            imports: self.imports.get_mut(),
            cycles: self.cycles.get_mut(),
        };
        graph::update(parts, before, after, sites)
    }
}

// What a kept state's store holds beyond its first scan: the scans, and the sites of the facts
// that later scans replaced, against the sites of the first scan.
struct Stored {
    scans: usize,
    replaced: usize,
    sites: usize,
}

impl Stored {
    fn of(files: &[&FileFacts]) -> Stored {
        Stored {
            scans: 1,
            replaced: 0,
            sites: files.iter().map(|file| weight(file)).sum(),
        }
    }

    // A scan of other paths replaces no file's facts; the update hands it to a full build.
    fn add(&mut self, before: &[&FileFacts], after: &[&FileFacts]) {
        self.scans += 1;
        if before.len() != after.len() {
            return;
        }
        let pairs = before.iter().zip(after);
        let replaced =
            pairs.filter(|(old, new)| !std::ptr::eq(**old, **new) && old.path == new.path);
        self.replaced += replaced.map(|(old, _)| weight(old)).sum::<usize>();
    }

    fn passes(&self, bounds: &Updates) -> bool {
        self.scans > bounds.scans || self.replaced * 100 > bounds.replaced * self.sites
    }
}

// The size of one file's facts, by its symbols and references.
fn weight(file: &FileFacts) -> usize {
    file.facts.symbols.len() + file.facts.refs.len()
}

// Gives `answer` the call graph of `files`, which builds nothing until an answer needs it.
pub(crate) fn with_call_graph<T>(
    files: &Arc<Vec<Arc<FileFacts>>>,
    deadline: Option<&Deadline>,
    keeping: Keeping<'_>,
    answer: impl FnOnce(&CallGraph<'_, '_>) -> T,
) -> T {
    let listed: Vec<&FileFacts> = files.iter().map(|file| &**file).collect();
    let built = Built::default();
    let resolver = OnceLock::new();
    answer(&CallGraph {
        files,
        listed: &listed,
        built: &built,
        resolver: &resolver,
        deadline,
        keeping,
    })
}

impl<'f, 's> CallGraph<'f, 's> {
    // `work` on the root's graph thread when the server keeps the graph, else on this call's own
    // graph. A thread that the system refuses leaves the work to the call, with the reason.
    pub(crate) fn over<R: Send + 'static>(
        &self,
        work: impl FnOnce(&CallGraph<'_, '_>) -> Result<R, Error> + Send + 'static,
    ) -> Noted<Result<R, Error>> {
        let Keeping::Server(graphs, root) = self.keeping else {
            return Noted::new(work(self));
        };
        let (job, replied) = Job::new(Arc::clone(self.files), work);
        let mut noted = Noted::new(());
        if let Err((job, refused)) = graphs.ask(root, job) {
            noted.diagnostics.push(refused);
            job.run(&CallGraph {
                keeping: Keeping::Own,
                ..*self
            });
        }
        noted.map(|()| awaited(&replied, self.deadline).map_err(Error::from)?)
    }

    pub(crate) fn files(&self) -> &'s [&'s FileFacts] {
        self.listed
    }

    // The call's deadline, or none on a graph thread, where the waiting call checks it.
    pub(crate) fn deadline(&self) -> Option<&'f Deadline> {
        self.deadline
    }

    pub(crate) fn index(&self) -> &'f NameIndex<'s> {
        self.built.index.get_or_init(|| {
            self.counted(|counts| &counts.indexes);
            NameIndex::build(self.listed)
        })
    }

    pub(crate) fn scopes(&self) -> &'f Scopes<'s> {
        self.built.scopes.get_or_init(|| {
            self.counted(|counts| &counts.scopes);
            Scopes::build(self.listed)
        })
    }

    pub(crate) fn resolver(&self) -> &'f Resolver<'f, 's> {
        self.resolver.get_or_init(|| {
            self.counted(|counts| &counts.resolvers);
            Resolver::new(self.index(), self.scopes())
        })
    }

    pub(crate) fn aliases(&self) -> &'f Aliases<'s> {
        (self.built.aliases).get_or_init(|| {
            self.counted(|counts| &counts.aliases);
            Aliases::build(self.resolver())
        })
    }

    // A stopped build stops every later answer too, as the deadline it ran under has passed.
    pub(crate) fn graph(&self) -> Result<&'f SymbolGraph<'s>, Error> {
        let built = self.built.graph.get_or_init(|| {
            self.counted(|counts| &counts.graphs);
            SymbolGraph::build(self.resolver(), self.deadline)
        });
        built.as_ref().map_err(|stopped| Error::from(*stopped))
    }

    pub(crate) fn degrees(&self) -> Result<&'f AmbiguousDegrees, Error> {
        let graph = self.graph()?;
        Ok(self.built.degrees.get_or_init(|| {
            self.counted(|counts| &counts.degrees);
            graph.ambiguous_degrees()
        }))
    }

    // The module-level import edges alone, which resolve no reference.
    pub(crate) fn imports(&self) -> &'f SymbolGraph<'s> {
        (self.built.imports).get_or_init(|| {
            self.counted(|counts| &counts.imports);
            SymbolGraph::imports(self.scopes())
        })
    }

    pub(crate) fn cycles(&self) -> &'f [Cycle<'s>] {
        (self.built.cycles).get_or_init(|| {
            self.counted(|counts| &counts.cycles);
            import_cycles(self.imports(), self.scopes())
        })
    }

    // Every part that a card reads, in the order that a symbol card first reads them.
    fn prebuild(&self) {
        let degrees = self.degrees();
        degrees.expect("a graph thread builds with no deadline, so no build of it stops");
        self.aliases();
        self.cycles();
    }

    fn counted(&self, count: impl FnOnce(&Counts) -> &AtomicUsize) {
        if let Keeping::Thread(counts) = self.keeping {
            count(counts).fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many times the kept graphs built each part, and updated them, over every root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Builds {
    /// The name indexes.
    pub indexes: usize,
    /// The import scopes.
    pub scopes: usize,
    /// The resolvers.
    pub resolvers: usize,
    /// The resolved graphs.
    pub graphs: usize,
    /// The updates of a kept graph to a later scan.
    pub updates: usize,
    /// The later scans that an update left to a full build.
    pub rebuilds: usize,
    /// The files whose every site the updates resolved again.
    pub resolved_files: usize,
    /// The sites that the updates resolved again.
    pub resolved_sites: usize,
}

// The counts of `Builds` and of the parts read from a graph, which the graph threads add to, and
// in a test the gate that holds back each prebuild.
#[derive(Debug, Default)]
pub(crate) struct Counts {
    indexes: AtomicUsize,
    scopes: AtomicUsize,
    resolvers: AtomicUsize,
    graphs: AtomicUsize,
    aliases: AtomicUsize,
    degrees: AtomicUsize,
    imports: AtomicUsize,
    cycles: AtomicUsize,
    updates: AtomicUsize,
    rebuilds: AtomicUsize,
    resolved_files: AtomicUsize,
    resolved_sites: AtomicUsize,
    // The scans that arrived while the prebuild of the scan before them still ran.
    waits: AtomicUsize,
    #[cfg(test)]
    gate: Option<Gate>,
    // The bounds of an update: the server's, or a test's.
    bounds: Updates,
}

impl Counts {
    fn updated(&self, work: Resolved) {
        self.updates.fetch_add(1, Ordering::Relaxed);
        self.resolved_files.fetch_add(work.files, Ordering::Relaxed);
        self.resolved_sites.fetch_add(work.sites, Ordering::Relaxed);
    }
}

// A scan that arrives while the prebuild runs waits for it, and a test hears of the wait.
fn waited(counts: &Counts) {
    counts.waits.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    if let Some(gate) = &counts.gate {
        let waiting = gate.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        let _unheard = waiting.send(());
    }
}

#[cfg(not(test))]
fn gated(_counts: &Counts, build: impl FnOnce()) {
    build();
}

// A test's prebuild waits until the test opens the gate, and says when it is done.
#[cfg(test)]
fn gated(counts: &Counts, build: impl FnOnce()) {
    let Some(gate) = &counts.gate else {
        return build();
    };
    let opened = gate
        .open
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .recv();
    opened.expect("the test keeps the gate while it waits for the prebuild");
    build();
    let done = gate.done.lock().unwrap_or_else(PoisonError::into_inner);
    done.send(()).expect("the test waits for the prebuild");
}

// What holds back a test's prebuild: one message on `open` starts one prebuild, and the prebuild
// sends one message on `done`. A scan that waits for a prebuild sends one message on `waiting`.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Gate {
    pub(crate) open: Mutex<Receiver<()>>,
    pub(crate) done: Mutex<Sender<()>>,
    pub(crate) waiting: Mutex<Sender<()>>,
}

/// The facts, name index and resolved graph of the roots that a server called last, kept between
/// its trace calls.
pub struct KeptGraphs {
    kept: Arc<Kept>,
    builds: Arc<Counts>,
}

impl std::fmt::Debug for KeptGraphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptGraphs")
            .field("roots", &self.kept_roots())
            .field("builds", &self.builds())
            .finish()
    }
}

// The kept roots, and the clock that their last calls are read on. A graph thread holds a weak
// handle, so that the state of an idle root can be dropped when no call arrives.
struct Kept {
    roots: Mutex<Roots>,
    clock: Arc<dyn Clock>,
}

#[derive(Default)]
struct Roots {
    by_path: HashMap<PathBuf, KeptRoot>,
    turns: u64,
}

// What a server keeps of one root: the facts of its last scan, its graph thread once a call asked
// for the graph, and its last call.
struct KeptRoot {
    facts: Arc<KeptFacts>,
    thread: Option<Sender<Job>>,
    used: Instant,
    turn: u64,
}

impl Kept {
    // The state of `root` for a call at `now`, after the retention rules dropped what they drop.
    fn called<'r>(
        &self,
        roots: &'r mut Roots,
        root: &Path,
        dropped: &mut Vec<KeptRoot>,
    ) -> &'r mut KeptRoot {
        let now = self.clock.now();
        dropped.extend(retired(roots, Some(root), now));
        roots.turns += 1;
        let turn = roots.turns;
        let kept = roots.by_path.entry(root.to_owned()).or_insert(KeptRoot {
            facts: Arc::default(),
            thread: None,
            used: now,
            turn,
        });
        (kept.used, kept.turn) = (now, turn);
        kept
    }

    // Drops the state of each root idle for the retention's time; the frees run off the lock.
    fn sweep(&self) {
        let dropped = retired(&mut self.roots(), None, self.clock.now());
        if !dropped.is_empty() {
            drop_detached(dropped);
        }
    }

    // No code panics while it holds the lock, so a poisoned map is still whole.
    fn roots(&self) -> MutexGuard<'_, Roots> {
        self.roots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// Removes the roots that the retention drops at `now`. A dropped root's thread ends after the
// jobs it holds, as its channel has no sender left.
fn retired(roots: &mut Roots, calling: Option<&Path>, now: Instant) -> Vec<KeptRoot> {
    let called: Vec<Called<'_>> = (roots.by_path.iter())
        .map(|(path, kept)| (path.as_path(), kept.used, kept.turn))
        .collect();
    let dropped = RETAINED.dropped(&called, calling, now);
    (dropped.iter())
        .filter_map(|path| roots.by_path.remove(path))
        .collect()
}

impl KeptGraphs {
    /// Returns a server's empty state, whose idle rule reads `clock`.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> KeptGraphs {
        let kept = Kept {
            roots: Mutex::default(),
            clock,
        };
        KeptGraphs {
            kept: Arc::new(kept),
            builds: Arc::default(),
        }
    }

    // A server's empty state whose prebuilds `gate` holds back.
    #[cfg(test)]
    pub(crate) fn gated(clock: Arc<dyn Clock>, gate: Gate) -> KeptGraphs {
        let builds = Counts {
            gate: Some(gate),
            ..Counts::default()
        };
        KeptGraphs::with_counts(clock, builds)
    }

    // A server's empty state whose updates keep `bounds`.
    #[cfg(test)]
    fn bounded(clock: Arc<dyn Clock>, bounds: Updates) -> KeptGraphs {
        let builds = Counts {
            bounds,
            ..Counts::default()
        };
        KeptGraphs::with_counts(clock, builds)
    }

    #[cfg(test)]
    fn with_counts(clock: Arc<dyn Clock>, builds: Counts) -> KeptGraphs {
        KeptGraphs {
            builds: Arc::new(builds),
            ..KeptGraphs::new(clock)
        }
    }

    /// Returns how many times the server built each part of a graph, over every root.
    #[must_use]
    pub fn builds(&self) -> Builds {
        let count = |counted: &AtomicUsize| counted.load(Ordering::Relaxed);
        let counts = &self.builds;
        Builds {
            indexes: count(&counts.indexes),
            scopes: count(&counts.scopes),
            resolvers: count(&counts.resolvers),
            graphs: count(&counts.graphs),
            updates: count(&counts.updates),
            rebuilds: count(&counts.rebuilds),
            resolved_files: count(&counts.resolved_files),
            resolved_sites: count(&counts.resolved_sites),
        }
    }

    // Sends `job` to the graph thread of `root`, started at the first job of the root or after
    // its thread ended. A thread that the system refuses hands the job back with the reason.
    fn ask(&self, root: &Path, job: Job) -> Result<(), (Job, String)> {
        self.with_root(root, |kept| self.sent(kept, root, job))
    }

    fn sent(&self, kept: &mut KeptRoot, root: &Path, job: Job) -> Result<(), (Job, String)> {
        let job = match &kept.thread {
            Some(thread) => match thread.send(job) {
                Ok(()) => return Ok(()),
                // The thread ended in a build that panicked; a new thread takes the job.
                Err(ended) => ended.0,
            },
            None => job,
        };
        let thread = match started(&self.builds, Arc::downgrade(&self.kept)) {
            Ok(thread) => thread,
            Err(error) => {
                let reason = format!(
                    "the graph thread of {} did not start: {error}",
                    root.display()
                );
                return Err((job, reason));
            }
        };
        let sent = thread.send(job);
        kept.thread = Some(thread);
        sent.map_err(|ended| {
            let reason = format!("the graph thread of {} ended", root.display());
            (ended.0, reason)
        })
    }

    // Starts the build of the graph of `files` on the graph thread of `root`, unless the thread
    // has started it. A refused thread refuses the call's next job too, which reports it.
    fn prebuild(&self, root: &Path, files: &Arc<Vec<Arc<FileFacts>>>) {
        let job = Job {
            files: Arc::clone(files),
            asked: Asked::Build,
        };
        let _refused = self.ask(root, job);
    }

    // The facts of the last scan of `root`, or none before its first scan.
    fn facts(&self, root: &Path) -> Arc<KeptFacts> {
        self.with_root(root, |kept| Arc::clone(&kept.facts))
    }

    fn keep(&self, root: &Path, facts: Arc<KeptFacts>) {
        self.with_root(root, |kept| kept.facts = facts);
    }

    // `work` over the state of `root` at a call. The state that the call drops is freed off the
    // lock, on a thread of its own.
    fn with_root<T>(&self, root: &Path, work: impl FnOnce(&mut KeptRoot) -> T) -> T {
        let mut dropped = Vec::new();
        let worked = work(self.kept.called(&mut self.kept.roots(), root, &mut dropped));
        if !dropped.is_empty() {
            drop_detached(dropped);
        }
        worked
    }

    // The kept roots, in path order.
    fn kept_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.kept.roots().by_path.keys().cloned().collect();
        roots.sort();
        roots
    }
}

// The work of a job: it answers over a call graph and sends the answer back.
type Work = Box<dyn FnOnce(&CallGraph<'_, '_>) + Send>;

// One question for a root's graph: the scan that the call read, and what the call asks of its
// graph.
struct Job {
    files: Arc<Vec<Arc<FileFacts>>>,
    asked: Asked,
}

enum Asked {
    // Work over the graph.
    Work(Work),
    // The build of every part of the graph, with no answer to wait for.
    Build,
}

// What arrives from a job: the work's outcome, or the payload of its panic.
type Replied<R> = Receiver<thread::Result<Result<R, Error>>>;

impl Job {
    fn new<R: Send + 'static>(
        files: Arc<Vec<Arc<FileFacts>>>,
        work: impl FnOnce(&CallGraph<'_, '_>) -> Result<R, Error> + Send + 'static,
    ) -> (Job, Replied<R>) {
        let (reply, replied) = mpsc::channel();
        let work = Box::new(move |graph: &CallGraph<'_, '_>| {
            let answered = panic::catch_unwind(AssertUnwindSafe(|| work(graph)));
            // A call that stopped has left, and nobody waits for its answer.
            drop(reply.send(answered));
        });
        let asked = Asked::Work(work);
        (Job { files, asked }, replied)
    }

    // A build that the call runs over its own graph is left to the call's work.
    fn run(self, graph: &CallGraph<'_, '_>) {
        if let Asked::Work(work) = self.asked {
            work(graph);
        }
    }
}

// The outcome of a job, or the stop of the call's deadline, checked while the call waits. A panic
// of the work goes on in the call's own thread.
fn awaited<R>(
    replied: &Replied<R>,
    deadline: Option<&Deadline>,
) -> Result<Result<R, Error>, Stopped> {
    loop {
        match replied.recv_timeout(WAIT) {
            Ok(Ok(answered)) => return Ok(answered),
            Ok(Err(payload)) => panic::resume_unwind(payload),
            Err(RecvTimeoutError::Timeout) => checkpoint(deadline)?,
            // Only a panic of a build drops a job unanswered; the graph thread printed it.
            Err(RecvTimeoutError::Disconnected) => panic!("the graph thread ended in a build"),
        }
    }
}

fn started(builds: &Arc<Counts>, kept: Weak<Kept>) -> std::io::Result<Sender<Job>> {
    let (thread, jobs) = mpsc::channel();
    let builds = Arc::clone(builds);
    let spawned = thread::Builder::new().name("agentless-graph".to_owned());
    spawned.spawn(move || served(&jobs, &builds, &kept))?;
    Ok(thread)
}

// The thread of one root: the jobs in order, until the server drops its end of the channel.
fn served(jobs: &Receiver<Job>, builds: &Counts, kept: &Weak<Kept>) {
    let thread = Jobs {
        queue: jobs,
        kept,
        builds,
    };
    let mut next = next_job(jobs, kept);
    while let Some(job) = next {
        next = thread.kept_state(job);
    }
}

// The next job. Each wait as long as the idle time with no job sweeps the server's roots, as no
// call arrives to do it.
fn next_job(jobs: &Receiver<Job>, kept: &Weak<Kept>) -> Option<Job> {
    loop {
        match jobs.recv_timeout(RETAINED.idle) {
            Ok(job) => return Some(job),
            Err(RecvTimeoutError::Timeout) => kept.upgrade()?.sweep(),
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

// The jobs of a graph thread, the weak handle that it sweeps the idle roots through, and the
// counts that it adds to.
struct Jobs<'j> {
    queue: &'j Receiver<Job>,
    kept: &'j Weak<Kept>,
    builds: &'j Counts,
}

impl Jobs<'_> {
    // One kept state: the scans that its parts borrow, declared first so that they drop last, and
    // the parts, which each update carries to the next scan.
    fn kept_state(&self, first: Job) -> Option<Job> {
        let store = Scan::new(&first.files);
        let mut built = Built::default();
        let mut stored = Stored::of(store.listed());
        let (mut scan, mut job) = (&store, first);
        loop {
            let next = self.scanned(job, scan, &built)?;
            let after = scan.then(&next.files);
            stored.add(scan.listed(), after.listed());
            let bounds = &self.builds.bounds;
            let updated = (!stored.passes(bounds))
                .then(|| built.updated(scan.listed(), after.listed(), bounds.sites));
            let Some(Ok(work)) = updated else {
                self.builds.rebuilds.fetch_add(1, Ordering::Relaxed);
                return Some(next);
            };
            self.builds.updated(work);
            (scan, job) = (after, next);
        }
    }

    // Answers each job of the first job's scan over `built`, and returns the first job of another
    // scan. The first build job starts the prebuild on a helper, which a later scan waits for.
    fn scanned<'s>(&self, first: Job, scan: &'s Scan<'s>, built: &Built<'s>) -> Option<Job> {
        let resolver = OnceLock::new();
        let graph = CallGraph {
            files: scan.files(),
            listed: scan.listed(),
            built,
            resolver: &resolver,
            deadline: None,
            keeping: Keeping::Thread(self.builds),
        };
        let finished = AtomicBool::new(false);
        let prebuild = || {
            gated(self.builds, || graph.prebuild());
            finished.store(true, Ordering::Release);
        };
        let next = thread::scope(|scope| {
            let mut prebuilding = None;
            let mut start = || {
                prebuilding.get_or_insert_with(|| io::spawn_or_run(scope, &prebuild));
            };
            let next = self.served(first, &graph, &mut start);
            if next.is_some() && prebuilding.is_some() && !finished.load(Ordering::Acquire) {
                waited(self.builds);
            }
            next
        });
        // A full build's resolver holds every resolution of the scan, which takes long to free.
        if let Some(resolver) = resolver.get() {
            drop_detached(resolver.take_answers());
        }
        next
    }

    // Runs `first` and each later job whose scan is the same, and returns the first job of
    // another scan.
    fn served(
        &self,
        first: Job,
        graph: &CallGraph<'_, '_>,
        start: &mut dyn FnMut(),
    ) -> Option<Job> {
        let mut job = first;
        // The scan of the last call that matched, so its other jobs skip the compare. A weak
        // handle keeps the allocation, so no later scan reuses its address.
        let mut matched: Weak<Vec<Arc<FileFacts>>> = Weak::new();
        loop {
            match job.asked {
                Asked::Work(work) => work(graph),
                Asked::Build => start(),
            }
            job = next_job(self.queue, self.kept)?;
            let known = Arc::ptr_eq(&job.files, graph.files)
                || std::ptr::eq(matched.as_ptr(), Arc::as_ptr(&job.files));
            if !known && !same_scan(&job.files, graph.files) {
                return Some(job);
            }
            matched = Arc::downgrade(&job.files);
        }
    }
}

// Two scans with the same facts for every file. A file whose facts both scans share is the same,
// and the scan threads compare the others.
fn same_scan(left: &[Arc<FileFacts>], right: &[Arc<FileFacts>]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let pairs = left.iter().zip(right);
    let differing: Vec<(&Arc<FileFacts>, &Arc<FileFacts>)> = (pairs)
        .filter(|(left, right)| !Arc::ptr_eq(left, right))
        .collect();
    let chunks: Vec<&[(&Arc<FileFacts>, &Arc<FileFacts>)]> =
        differing.chunks(COMPARED_FILES).collect();
    let same = map_in_order(&chunks, threads(), |chunk| {
        chunk.iter().all(|(left, right)| ***left == ***right)
    });
    same.into_iter().all(|same| same)
}

#[cfg(test)]
mod tests;
