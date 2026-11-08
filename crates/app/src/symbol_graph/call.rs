//! The name index and the resolved graph that one call reads, each built at its first use, and the
//! facts and graphs that a server keeps between its calls. A kept graph borrows the scan it was
//! built from, so it lives on the stack of one thread per root, and a call sends that thread a job
//! that owns everything it reads. A server's call starts the build of its scan's graph on a helper
//! of that thread, so a later card finds it built.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use git::RepoFile;
use graph::{
    Aliases, AmbiguousDegrees, Cycle, FileFacts, NameIndex, Resolver, Scopes, SymbolGraph,
    import_cycles,
};
use kernel::{Clock, Deadline, Stopped, checkpoint};

use super::retention::{Called, RETAINED};
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

// The name index, import scopes, resolver and resolved graph of one scan. The first answer that
// needs one builds it, and every later answer of the call reads the same one.
#[derive(Clone, Copy)]
pub(crate) struct CallGraph<'a> {
    files: &'a Arc<Vec<Arc<FileFacts>>>,
    // The facts side by side, which every part borrows, copied at the first build of a part.
    built_over: &'a OnceLock<Vec<FileFacts>>,
    built: &'a Built<'a>,
    deadline: Option<&'a Deadline>,
    keeping: Keeping<'a>,
}

// What a call graph builds from its files. A lock holds each part, so that a graph thread and its
// prebuild share them and build each once.
#[derive(Default)]
struct Built<'a> {
    index: OnceLock<NameIndex<'a>>,
    scopes: OnceLock<Scopes<'a>>,
    resolver: OnceLock<Resolver<'a>>,
    aliases: OnceLock<Aliases<'a>>,
    graph: OnceLock<Result<SymbolGraph<'a>, Stopped>>,
    degrees: OnceLock<AmbiguousDegrees>,
    imports: OnceLock<SymbolGraph<'a>>,
    cycles: OnceLock<Vec<Cycle<'a>>>,
}

// Gives `answer` the call graph of `files`, which builds nothing until an answer needs it.
pub(crate) fn with_call_graph<T>(
    files: &Arc<Vec<Arc<FileFacts>>>,
    deadline: Option<&Deadline>,
    keeping: Keeping<'_>,
    answer: impl FnOnce(&CallGraph<'_>) -> T,
) -> T {
    let built_over = OnceLock::new();
    let built = Built::default();
    answer(&CallGraph {
        files,
        built_over: &built_over,
        built: &built,
        deadline,
        keeping,
    })
}

impl<'a> CallGraph<'a> {
    // `work` on the root's graph thread when the server keeps the graph, else on this call's own
    // graph. A thread that the system refuses leaves the work to the call, with the reason.
    pub(crate) fn over<R: Send + 'static>(
        &self,
        work: impl FnOnce(&CallGraph<'_>) -> Result<R, Error> + Send + 'static,
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

    pub(crate) fn files(&self) -> &'a [Arc<FileFacts>] {
        self.files
    }

    fn built_over(&self) -> &'a [FileFacts] {
        let files = self.files;
        self.built_over
            .get_or_init(|| files.iter().map(|file| FileFacts::clone(file)).collect())
    }

    // The call's deadline, or none on a graph thread, where the waiting call checks it.
    pub(crate) fn deadline(&self) -> Option<&'a Deadline> {
        self.deadline
    }

    pub(crate) fn index(&self) -> &'a NameIndex<'a> {
        self.built.index.get_or_init(|| {
            self.counted(|counts| &counts.indexes);
            NameIndex::build(self.built_over())
        })
    }

    pub(crate) fn scopes(&self) -> &'a Scopes<'a> {
        self.built.scopes.get_or_init(|| {
            self.counted(|counts| &counts.scopes);
            Scopes::build(self.built_over())
        })
    }

    pub(crate) fn resolver(&self) -> &'a Resolver<'a> {
        self.built.resolver.get_or_init(|| {
            self.counted(|counts| &counts.resolvers);
            Resolver::new(self.index(), self.scopes())
        })
    }

    pub(crate) fn aliases(&self) -> &'a Aliases<'a> {
        (self.built.aliases).get_or_init(|| {
            self.counted(|counts| &counts.aliases);
            Aliases::build(self.resolver())
        })
    }

    // A stopped build stops every later answer too, as the deadline it ran under has passed.
    pub(crate) fn graph(&self) -> Result<&'a SymbolGraph<'a>, Error> {
        let built = self.built.graph.get_or_init(|| {
            self.counted(|counts| &counts.graphs);
            SymbolGraph::build(self.resolver(), self.deadline)
        });
        built.as_ref().map_err(|stopped| Error::from(*stopped))
    }

    pub(crate) fn degrees(&self) -> Result<&'a AmbiguousDegrees, Error> {
        let graph = self.graph()?;
        Ok(self.built.degrees.get_or_init(|| {
            self.counted(|counts| &counts.degrees);
            graph.ambiguous_degrees()
        }))
    }

    // The module-level import edges alone, which resolve no reference.
    pub(crate) fn imports(&self) -> &'a SymbolGraph<'a> {
        (self.built.imports).get_or_init(|| {
            self.counted(|counts| &counts.imports);
            SymbolGraph::imports(self.scopes())
        })
    }

    pub(crate) fn cycles(&self) -> &'a [Cycle<'a>] {
        (self.built.cycles).get_or_init(|| {
            self.counted(|counts| &counts.cycles);
            import_cycles(self.imports())
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

/// How many times the kept graphs built each part, over every root.
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
    #[cfg(test)]
    gate: Option<Gate>,
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
// sends one message on `done`.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Gate {
    pub(crate) open: Mutex<Receiver<()>>,
    pub(crate) done: Mutex<Sender<()>>,
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
        let graphs = KeptGraphs::new(clock);
        let builds = Counts {
            gate: Some(gate),
            ..Counts::default()
        };
        KeptGraphs {
            builds: Arc::new(builds),
            ..graphs
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
type Work = Box<dyn FnOnce(&CallGraph<'_>) + Send>;

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
        work: impl FnOnce(&CallGraph<'_>) -> Result<R, Error> + Send + 'static,
    ) -> (Job, Replied<R>) {
        let (reply, replied) = mpsc::channel();
        let work = Box::new(move |graph: &CallGraph<'_>| {
            let answered = panic::catch_unwind(AssertUnwindSafe(|| work(graph)));
            // A call that stopped has left, and nobody waits for its answer.
            drop(reply.send(answered));
        });
        let asked = Asked::Work(work);
        (Job { files, asked }, replied)
    }

    // A build that the call runs over its own graph is left to the call's work.
    fn run(self, graph: &CallGraph<'_>) {
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
    let mut next = next_job(jobs, kept);
    while let Some(job) = next {
        next = held(job, jobs, builds, kept);
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

// Answers each job of the first job's scan over one graph, and returns the first job of another
// scan. The first build job starts the prebuild on a helper, which a later scan waits for.
fn held(first: Job, jobs: &Receiver<Job>, builds: &Counts, kept: &Weak<Kept>) -> Option<Job> {
    let files = Arc::clone(&first.files);
    with_call_graph(&files, None, Keeping::Thread(builds), |graph| {
        let prebuild = || gated(builds, || graph.prebuild());
        thread::scope(|scope| {
            let mut prebuilding = None;
            let mut build = || {
                prebuilding.get_or_insert_with(|| io::spawn_or_run(scope, &prebuild));
            };
            let scan = Scan {
                jobs,
                kept,
                files: &files,
            };
            scan.served(first, graph, &mut build)
        })
    })
}

// The jobs of one scan, and the weak handle that a graph thread sweeps the idle roots through.
struct Scan<'s> {
    jobs: &'s Receiver<Job>,
    kept: &'s Weak<Kept>,
    files: &'s Arc<Vec<Arc<FileFacts>>>,
}

impl Scan<'_> {
    // Runs `first` and each later job whose scan is the same, and returns the first job of
    // another scan.
    fn served(&self, first: Job, graph: &CallGraph<'_>, build: &mut dyn FnMut()) -> Option<Job> {
        let mut job = first;
        // The scan of the last call that matched, so its other jobs skip the compare. A weak
        // handle keeps the allocation, so no later scan reuses its address.
        let mut matched: Weak<Vec<Arc<FileFacts>>> = Weak::new();
        loop {
            match job.asked {
                Asked::Work(work) => work(graph),
                Asked::Build => build(),
            }
            job = next_job(self.jobs, self.kept)?;
            let known = Arc::ptr_eq(&job.files, self.files)
                || std::ptr::eq(matched.as_ptr(), Arc::as_ptr(&job.files));
            if !known && !same_scan(&job.files, self.files) {
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
