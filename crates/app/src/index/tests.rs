use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, ThreadId};
use std::time::{Duration, UNIX_EPOCH};

use store::{Extract, FileText};
use syntax::Language;

use super::{IndexRequest, Parser, index, index_with};
use crate::clock::PinnedClock;
use crate::testing::Sandbox;
use crate::{Error, IndexReport, Noted, Repo, Services, Session};

mod properties;
mod scenarios;

const WALL_SECONDS: u64 = 1_800_000_000;

fn services(sandbox: &Sandbox) -> Services {
    let mut services = sandbox.services(sandbox.path());
    services.clock = Arc::new(PinnedClock::at(
        UNIX_EPOCH + Duration::from_secs(WALL_SECONDS),
    ));
    services
}

fn snapshot(services: &Services, root: &Path) -> git::Snapshot {
    services
        .git
        .snapshot(root, services.git.root(root).as_deref())
}

fn run(services: &Services, root: &Path, force: bool) -> Noted<Result<IndexReport, Error>> {
    let repo = Repo::load(services, root).expect("the repository loads");
    let request = IndexRequest {
        force,
        max_cache_bytes: None,
    };
    index(services, &repo, &snapshot(services, root), &request)
}

fn indexed(services: &Services, root: &Path, force: bool) -> IndexReport {
    let noted = run(services, root, force);
    assert!(noted.diagnostics.is_empty(), "{:?}", noted.diagnostics);
    noted.value.expect("the index run succeeds")
}

// indexed, reused, pruned, skipped, errors, and the files that the database holds.
fn numbers(report: &IndexReport) -> [u64; 6] {
    let count = |value: usize| u64::try_from(value).expect("a small count");
    [
        count(report.indexed),
        count(report.reused),
        count(report.pruned),
        count(report.skipped()),
        count(report.errors()),
        report.files,
    ]
}

const PYTHON: &str =
    "import os\n\n\nclass Box:\n    def size(self, x):\n        return os.sep + x\n";
const SCRIPT: &str = "export function add(a, b) {\n  return a + b;\n}\n";

#[test]
fn a_warm_run_reuses_rows_and_each_change_reextracts_only_its_file() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("tree");
    sandbox.put("tree/a.py", PYTHON);
    sandbox.put("tree/web/b.js", SCRIPT);
    sandbox.put("tree/notes.md", "# no grammar\n");
    let services = services(&sandbox);

    assert_eq!(
        numbers(&indexed(&services, &root, false)),
        [2, 0, 0, 0, 0, 2]
    );
    assert_eq!(
        numbers(&indexed(&services, &root, false)),
        [0, 2, 0, 0, 0, 2]
    );
    sandbox.put("tree/a.py", "x = 1\n");
    assert_eq!(
        numbers(&indexed(&services, &root, false)),
        [1, 1, 0, 0, 0, 2]
    );
    io::remove_file(&root.join("web/b.js")).expect("the file is removed");
    assert_eq!(
        numbers(&indexed(&services, &root, false)),
        [0, 1, 1, 0, 0, 1]
    );
    assert_eq!(
        numbers(&indexed(&services, &root, true)),
        [1, 0, 0, 0, 0, 1]
    );
}

#[test]
fn the_report_names_the_database_and_the_generation() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", PYTHON)]);
    let services = services(&sandbox);

    let report = indexed(&services, &root, false);

    let tree = snapshot(&services, &root).tree_oid.expect("a tree id");
    assert_eq!(report.generation, tree);
    assert_eq!(report.database.file_name(), Some("agentless.db".as_ref()));
    assert_eq!((report.tags, report.imports), (2, 1));
    assert!(report.refs > 0);
}

// The session over agentless.db serves each file as one parse of its text gives it.
#[test]
fn a_read_after_the_run_serves_what_a_parse_gives() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", PYTHON), ("b.js", SCRIPT)]);
    let services = services(&sandbox);
    indexed(&services, &root, false);

    let repo = Repo::load(&services, &root).expect("the repository loads");
    let session = Session::open_facts(&services, repo, false).session.value;
    let session = session.expect("a session");

    assert!(
        session.cache.receipt.ends_with(" fresh"),
        "{}",
        session.cache.receipt
    );
    assert_eq!(
        session.cache.counts.as_ref().map(|counts| counts.files),
        Ok(2)
    );
    for path in ["a.py", "b.js"] {
        let file = text_of(&root, path);
        let parsed = store::analyze(&file, true).expect("the file parses");
        let facts = session.facts.facts(&file);
        let metrics = session.facts.metrics(&file);
        assert!(facts.warning.is_none() && metrics.warning.is_none());
        assert_eq!(facts.value.as_ref(), Ok(&parsed.facts), "{path}");
        assert_eq!(Some(metrics.value), parsed.metrics, "{path}");
    }
}

fn text_of(root: &Path, path: &str) -> FileText {
    let bytes = io::read_bounded(&root.join(path)).expect("the file reads");
    let language = Language::from_path(path).expect("a known language");
    FileText::decode(path.to_owned(), language, &bytes)
}

#[test]
fn a_held_lock_refuses_the_run_and_names_the_repository() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("tree");
    sandbox.put("tree/a.py", PYTHON);
    let services = services(&sandbox);
    let database = indexed(&services, &root, false).database;
    let directory = database.parent().expect("a key directory");
    let held = io::try_lock(&directory.join("agentless.lock")).expect("the lock is free");

    let refused = run(&services, &root, false)
        .value
        .expect_err("the lock is held");

    let expected = format!(
        "could not take the index lock for {}: another index run may hold it, or {} may be on a \
         filesystem that does not support locking",
        root.display(),
        directory.display()
    );
    assert!(matches!(
        &refused,
        Error::Kind(kernel::Error::CacheLocked(_))
    ));
    assert_eq!(refused.to_string(), expected);
    drop(held);
    assert_eq!(
        numbers(&indexed(&services, &root, false)),
        [0, 1, 0, 0, 0, 1]
    );
}

// A file that is not a database stops the run, as the reference build does. A read discards it.
#[test]
fn a_file_that_is_not_a_database_refuses_the_run() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("tree");
    sandbox.put("tree/a.py", PYTHON);
    let services = services(&sandbox);
    let database = indexed(&services, &root, false).database;
    for suffix in ["", "-wal", "-shm"] {
        let mut path = database.clone().into_os_string();
        path.push(suffix);
        io::remove_file(Path::new(&path)).expect("the file is removed");
    }
    let relative = database
        .strip_prefix(sandbox.path())
        .expect("a sandbox path");
    sandbox.put(
        relative.to_str().expect("UTF-8"),
        "not a database\n".repeat(64),
    );

    let refused = run(&services, &root, false)
        .value
        .expect_err("the file is refused");

    let expected = format!(
        "cannot build the tag cache for {}: file is not a database",
        root.display()
    );
    assert_eq!(refused.to_string(), expected);
}

// A database whose header reads and whose meta table does not is removed and built again. Page 2
// is the meta table's root, because the schema creates that table first.
#[test]
fn a_database_whose_tables_do_not_read_is_built_again() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("tree");
    sandbox.put("tree/a.py", PYTHON);
    let services = services(&sandbox);
    let database = indexed(&services, &root, false).database;
    let mut bytes = std::fs::read(&database).expect("the database reads");
    bytes[4096..8192].fill(0xa5);
    let relative = database
        .strip_prefix(sandbox.path())
        .expect("a sandbox path");
    sandbox.put(relative.to_str().expect("UTF-8"), bytes);

    let report = indexed(&services, &root, false);

    assert_eq!(numbers(&report), [1, 0, 0, 0, 0, 1]);
}

// A file cut to its first page fails the connect, as it does in the reference build.
#[test]
fn a_database_cut_to_its_first_page_refuses_the_run() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("tree");
    sandbox.put("tree/a.py", PYTHON);
    let services = services(&sandbox);
    let database = indexed(&services, &root, false).database;
    let head = std::fs::read(&database).expect("the database reads")[..4096].to_vec();
    let relative = database
        .strip_prefix(sandbox.path())
        .expect("a sandbox path");
    sandbox.put(relative.to_str().expect("UTF-8"), head);

    let refused = run(&services, &root, false)
        .value
        .expect_err("the file is refused");

    let expected = format!(
        "cannot build the tag cache for {}: database disk image is malformed",
        root.display()
    );
    assert_eq!(refused.to_string(), expected);
}

// One pool thread gives the run that a refused helper thread leaves: the same report and rows.
#[test]
fn one_thread_and_eight_threads_give_the_same_database() {
    let runs: Vec<(IndexReport, Vec<String>)> = [1, 8]
        .into_iter()
        .map(|threads| {
            let sandbox = Sandbox::new();
            let root = corpus(&sandbox);
            let services = services(&sandbox);
            let parse = |planned: &Extract| store::analyze(&planned.file, planned.measure);
            let parser = Parser {
                threads,
                parse: &parse,
            };
            let report = with_parser(&services, &root, false, &parser).expect("the run succeeds");
            (report, served(&services, &root))
        })
        .collect();
    let [(one, one_rows), (eight, eight_rows)] = <[_; 2]>::try_from(runs).expect("two runs");
    assert_eq!(numbers(&one), numbers(&eight));
    assert_eq!(
        (one.tags, one.imports, one.refs),
        (eight.tags, eight.imports, eight.refs)
    );
    assert_eq!(one_rows, eight_rows);
}

fn with_parser(
    services: &Services,
    root: &Path,
    force: bool,
    parser: &Parser<'_>,
) -> Result<IndexReport, Error> {
    let repo = Repo::load(services, root).expect("the repository loads");
    let request = IndexRequest {
        force,
        max_cache_bytes: None,
    };
    index_with(services, &repo, &snapshot(services, root), &request, parser).value
}

// Thirteen files of four languages, one of them with a syntax error.
fn corpus(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.mkdir("tree");
    for index in 0..3 {
        sandbox.put(
            &format!("tree/p{index}.py"),
            format!("{PYTHON}v{index} = {index}\n"),
        );
        sandbox.put(
            &format!("tree/s{index}.js"),
            format!("{SCRIPT}// {index}\n"),
        );
        sandbox.put(
            &format!("tree/g{index}.go"),
            format!("package p\n\nfunc F{index}() {{}}\n"),
        );
        sandbox.put(
            &format!("tree/d{index}.json"),
            format!("{{\"k{index}\": {index}}}\n"),
        );
    }
    sandbox.put("tree/broken.py", "def broken(:\n    pass\n");
    root
}

// Each file's served facts and metrics, as debug text, in path order.
fn served(services: &Services, root: &Path) -> Vec<String> {
    let repo = Repo::load(services, root).expect("the repository loads");
    let session = Session::open_facts(services, repo, false).session.value;
    let session = session.expect("a session");
    let mut paths: Vec<String> = walked(services, root);
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let file = text_of(root, path);
            let facts = session.facts.facts(&file);
            let metrics = session.facts.metrics(&file);
            format!("{path} {:?} {:?}", facts.value, metrics.value)
        })
        .collect()
}

fn walked(services: &Services, root: &Path) -> Vec<String> {
    let files = services.git.walk(root).files.expect("the walk succeeds");
    files
        .into_iter()
        .map(|file| file.path)
        .filter(|path| Language::from_path(path).is_some())
        .collect()
}

// A panic in a parse on a helper thread reaches the caller with its payload. Nothing is written,
// and the lock is free again.
#[test]
fn a_panic_on_a_helper_thread_reaches_the_caller_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let root = corpus(&sandbox);
    let services = services(&sandbox);
    let database = indexed(&services, &root, false).database;
    let before = std::fs::read(&database).expect("the database reads");
    let caller: ThreadId = thread::current().id();
    let parse = |planned: &Extract| {
        if thread::current().id() == caller {
            // The caller waits, so the helpers claim the other files.
            thread::sleep(Duration::from_millis(50));
            return store::analyze(&planned.file, planned.measure);
        }
        panic!("a helper parse broke on {}", planned.file.path);
    };
    let parser = Parser {
        threads: 4,
        parse: &parse,
    };

    let caught = panic::catch_unwind(AssertUnwindSafe(|| {
        with_parser(&services, &root, true, &parser)
    }));

    let payload = caught.expect_err("the panic reaches the caller");
    let message = payload
        .downcast_ref::<String>()
        .expect("a formatted message");
    assert!(message.starts_with("a helper parse broke on "), "{message}");
    assert_eq!(
        std::fs::read(&database).expect("the database reads"),
        before
    );
    assert_eq!(
        numbers(&indexed(&services, &root, false)),
        [0, 13, 0, 0, 0, 13]
    );
}
