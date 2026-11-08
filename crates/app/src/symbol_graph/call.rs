//! The name index and the resolved graph that one call reads, each built at its first use, and the
//! facts and graphs that a server keeps between its calls. A kept graph borrows the scan it was
//! built from, so it lives on the stack of one thread per root, and a call sends that thread a job
//! that owns everything it reads.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::Duration;

use git::RepoFile;
use graph::{
    AmbiguousDegrees, Cycle, FileFacts, NameIndex, Resolver, Scopes, SymbolGraph, import_cycles,
};
use kernel::{Deadline, Stopped, checkpoint};

use crate::detached::answered_over;
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
    pub(crate) files: Arc<Vec<FileFacts>>,
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
            files: Arc::new(facts.files),
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
                    (files, skipped)
                })
            })
        }
        Keeping::Own | Keeping::Thread(_) => {
            let scanned = scan_walked(root, files, source, deadline);
            scanned.map(|scanned| scanned.map(|facts| (Arc::new(facts.files), facts.skipped)))
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
    files: &'a Arc<Vec<FileFacts>>,
    built: &'a Built<'a>,
    deadline: Option<&'a Deadline>,
    keeping: Keeping<'a>,
}

// What a call graph builds from its files, and what it reads from them once: the ambiguous
// degrees of the graph, and the import cycles of the import edges. Each borrows the files.
#[derive(Default)]
struct Built<'a> {
    index: OnceCell<NameIndex<'a>>,
    scopes: OnceCell<Scopes<'a>>,
    resolver: OnceCell<Resolver<'a>>,
    graph: OnceCell<Result<SymbolGraph<'a>, Stopped>>,
    degrees: OnceCell<AmbiguousDegrees>,
    imports: OnceCell<SymbolGraph<'a>>,
    cycles: OnceCell<Vec<Cycle<'a>>>,
}

// Gives `answer` the call graph of `files`, which builds nothing until an answer needs it.
pub(crate) fn with_call_graph<T>(
    files: &Arc<Vec<FileFacts>>,
    deadline: Option<&Deadline>,
    keeping: Keeping<'_>,
    answer: impl FnOnce(&CallGraph<'_>) -> T,
) -> T {
    let built = Built::default();
    answer(&CallGraph {
        files,
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

    pub(crate) fn files(&self) -> &'a [FileFacts] {
        self.files
    }

    // The call's deadline, or none on a graph thread, where the waiting call checks it.
    pub(crate) fn deadline(&self) -> Option<&'a Deadline> {
        self.deadline
    }

    pub(crate) fn index(&self) -> &'a NameIndex<'a> {
        let files = self.files;
        self.built.index.get_or_init(|| {
            self.counted(|counts| &counts.indexes);
            NameIndex::build(files)
        })
    }

    pub(crate) fn scopes(&self) -> &'a Scopes<'a> {
        let files = self.files;
        self.built.scopes.get_or_init(|| {
            self.counted(|counts| &counts.scopes);
            Scopes::build(files)
        })
    }

    pub(crate) fn resolver(&self) -> &'a Resolver<'a> {
        self.built.resolver.get_or_init(|| {
            self.counted(|counts| &counts.resolvers);
            Resolver::new(self.index(), self.scopes())
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
        Ok(self.built.degrees.get_or_init(|| graph.ambiguous_degrees()))
    }

    // The module-level import edges alone, which resolve no reference.
    pub(crate) fn imports(&self) -> &'a SymbolGraph<'a> {
        (self.built.imports).get_or_init(|| SymbolGraph::imports(self.scopes()))
    }

    pub(crate) fn cycles(&self) -> &'a [Cycle<'a>] {
        (self.built.cycles).get_or_init(|| import_cycles(self.imports()))
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

// The counts of `Builds`, which the graph threads add to.
#[derive(Debug, Default)]
pub(crate) struct Counts {
    indexes: AtomicUsize,
    scopes: AtomicUsize,
    resolvers: AtomicUsize,
    graphs: AtomicUsize,
}

/// The facts, name index and resolved graph of each root, kept between a server's trace calls.
#[derive(Default)]
pub struct KeptGraphs {
    roots: Mutex<HashMap<PathBuf, KeptRoot>>,
    builds: Arc<Counts>,
}

impl std::fmt::Debug for KeptGraphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let roots: Vec<PathBuf> = self.roots().keys().cloned().collect();
        f.debug_struct("KeptGraphs")
            .field("roots", &roots)
            .field("builds", &self.builds())
            .finish()
    }
}

// What a server keeps of one root: the facts of its last scan, and its graph thread once a call
// asked for the graph.
#[derive(Default)]
struct KeptRoot {
    facts: Arc<KeptFacts>,
    thread: Option<Sender<Job>>,
}

impl KeptGraphs {
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
        let mut roots = self.roots();
        let kept = roots.entry(root.to_owned()).or_default();
        let job = match &kept.thread {
            Some(thread) => match thread.send(job) {
                Ok(()) => return Ok(()),
                // The thread ended in a build that panicked; a new thread takes the job.
                Err(ended) => ended.0,
            },
            None => job,
        };
        let thread = match started(&self.builds) {
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

    // The facts of the last scan of `root`, or none before its first scan.
    fn facts(&self, root: &Path) -> Arc<KeptFacts> {
        let roots = self.roots();
        roots
            .get(root)
            .map(|kept| Arc::clone(&kept.facts))
            .unwrap_or_default()
    }

    fn keep(&self, root: &Path, facts: Arc<KeptFacts>) {
        self.roots().entry(root.to_owned()).or_default().facts = facts;
    }

    // No code panics while it holds the lock, so a poisoned map is still whole.
    fn roots(&self) -> MutexGuard<'_, HashMap<PathBuf, KeptRoot>> {
        self.roots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// The work of a job: it answers over a call graph and sends the answer back.
type Work = Box<dyn FnOnce(&CallGraph<'_>) + Send>;

// One question for a root's graph: the scan that the call read, and the work over its graph.
struct Job {
    files: Arc<Vec<FileFacts>>,
    work: Work,
}

// What arrives from a job: the work's outcome, or the payload of its panic.
type Replied<R> = Receiver<thread::Result<Result<R, Error>>>;

impl Job {
    fn new<R: Send + 'static>(
        files: Arc<Vec<FileFacts>>,
        work: impl FnOnce(&CallGraph<'_>) -> Result<R, Error> + Send + 'static,
    ) -> (Job, Replied<R>) {
        let (reply, replied) = mpsc::channel();
        let work = Box::new(move |graph: &CallGraph<'_>| {
            let answered = panic::catch_unwind(AssertUnwindSafe(|| work(graph)));
            // A call that stopped has left, and nobody waits for its answer.
            drop(reply.send(answered));
        });
        (Job { files, work }, replied)
    }

    fn run(self, graph: &CallGraph<'_>) {
        (self.work)(graph);
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

fn started(builds: &Arc<Counts>) -> std::io::Result<Sender<Job>> {
    let (thread, jobs) = mpsc::channel();
    let builds = Arc::clone(builds);
    let spawned = thread::Builder::new().name("agentless-graph".to_owned());
    spawned.spawn(move || served(&jobs, &builds))?;
    Ok(thread)
}

// The thread of one root: the jobs in order, until the server drops its end of the channel.
fn served(jobs: &Receiver<Job>, builds: &Counts) {
    let mut next = jobs.recv().ok();
    while let Some(job) = next {
        next = held(job, jobs, builds);
    }
}

// Answers the first job and each later job whose scan is the same over one graph, and returns
// the first job whose scan differs. No deadline: a call that stops leaves its build to the next.
fn held(first: Job, jobs: &Receiver<Job>, builds: &Counts) -> Option<Job> {
    let files = Arc::clone(&first.files);
    with_call_graph(&files, None, Keeping::Thread(builds), |graph| {
        first.run(graph);
        // The scan of the last call that matched, so its other jobs skip the compare. A weak
        // handle keeps the allocation, so no later scan reuses its address.
        let mut matched: Weak<Vec<FileFacts>> = Weak::new();
        loop {
            let job = jobs.recv().ok()?;
            let known = Arc::ptr_eq(&job.files, &files)
                || std::ptr::eq(matched.as_ptr(), Arc::as_ptr(&job.files));
            if !known && !same_scan(&job.files, &files) {
                return Some(job);
            }
            matched = Arc::downgrade(&job.files);
            job.run(graph);
        }
    })
}

// Two scans with the same facts for every file, compared on the scan threads.
fn same_scan(left: &[FileFacts], right: &[FileFacts]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let starts: Vec<usize> = (0..left.len()).step_by(COMPARED_FILES).collect();
    let same = map_in_order(&starts, threads(), |&start| {
        let end = (start + COMPARED_FILES).min(left.len());
        left[start..end] == right[start..end]
    });
    same.into_iter().all(|same| same)
}
