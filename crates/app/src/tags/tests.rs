use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use store::CacheState;

use super::{Session, Tags, outside, settled};
use crate::testing::{Sandbox, tool};
use crate::{Noted, Repo, Services};

// The repository root and the generation that the store crate's fixture database records.
const FIXTURE_ROOT: &str = "/agentless-store-fixture/repo";
const FIXTURE_GENERATION: &str = "348a75f0";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../store/fixtures/tags.db")
}

fn modified(path: &Path) -> SystemTime {
    let meta = std::fs::metadata(path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    meta.modified().expect("a modification time")
}

// Sets the modification time of `path` far in the past, so that a stamp shows.
fn age(path: &Path) {
    let path_variable = std::env::var_os("PATH").expect("the tests find touch through PATH");
    let args = vec![
        OsString::from("-d"),
        OsString::from("2000-01-01T00:00:00Z"),
        path.as_os_str().to_owned(),
    ];
    tool("touch", args, vec![(OsString::from("PATH"), path_variable)]);
}

fn aged_copy(sandbox: &Sandbox, relative: &str) -> (PathBuf, SystemTime) {
    let database = sandbox.copy(&fixture(), relative);
    age(&database);
    let before = modified(&database);
    (database, before)
}

#[test]
fn a_usable_database_gets_its_generation_then_the_stamp() {
    let sandbox = Sandbox::new();
    let (database, before) = aged_copy(&sandbox, "cache/tags.db");
    let services = sandbox.services(sandbox.path());
    let source = store::open_source(&database, FIXTURE_ROOT, false);

    let tree = Some(FIXTURE_GENERATION);
    let (state, tags) =
        settled(&services, sandbox.path(), tree, source, &database).expect("a generation");

    assert!(tags.cached());
    assert!(modified(&database) > before);
    let expected = CacheState::Indexed {
        generation: FIXTURE_GENERATION.to_owned(),
        repo_generation: FIXTURE_GENERATION.to_owned(),
        repo_root: sandbox.path().display().to_string(),
    };
    assert_eq!(state, expected);
}

#[test]
fn a_bypassed_database_gets_no_stamp() {
    let sandbox = Sandbox::new();
    let (database, before) = aged_copy(&sandbox, "cache/tags.db");
    let services = sandbox.services(sandbox.path());
    let source = store::open_source(&database, FIXTURE_ROOT, true);

    let (state, tags) =
        settled(&services, sandbox.path(), None, source, &database).expect("no generation");

    assert!(!tags.cached());
    assert_eq!(modified(&database), before);
    assert_eq!(state, CacheState::Bypassed);
}

#[test]
fn a_usable_database_outside_git_reads_the_generation_of_the_walked_files() {
    let sandbox = Sandbox::new();
    let (database, _) = aged_copy(&sandbox, "cache/tags.db");
    sandbox.put("tree/a.py", "x = 1\n");
    let services = sandbox.services(sandbox.path());
    let source = store::open_source(&database, FIXTURE_ROOT, false);
    let root = sandbox.path().join("tree");

    let (state, _) = settled(&services, &root, None, source, &database).expect("a walk");

    let CacheState::Indexed {
        repo_generation, ..
    } = state
    else {
        panic!("a usable database, not {state:?}");
    };
    assert!(repo_generation.starts_with("nogit:"), "{repo_generation}");
    assert_eq!(repo_generation.len(), "nogit:".len() + 16);
}

fn opened(services: &Services, root: &Path, no_cache: bool) -> Noted<Session> {
    let repo = crate::resolve(services, Some(root)).expect("the repository resolves");
    Session::open(services, repo, no_cache)
        .session
        .expect("a session")
}

// Copies the fixture database, which another repository built, to the database path of `root`.
fn foreign_database(sandbox: &Sandbox, services: &Services, root: &Path) -> PathBuf {
    let cache_root = services.cache_root.as_ref().expect("a cache root");
    let database = store::database_path(&cache_root.path, root.to_str().expect("a UTF-8 path"));
    let relative = database
        .strip_prefix(sandbox.path())
        .expect("a sandbox path");
    sandbox.copy(&fixture(), relative.to_str().expect("a UTF-8 path"))
}

#[test]
fn a_database_built_for_another_repository_is_discarded_and_named_in_the_receipt() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let services = sandbox.services(sandbox.path());
    let database = foreign_database(&sandbox, &services, &root);

    let session = opened(&services, &root, false);

    let receipt = format!("none (cache discarded: it was built for {FIXTURE_ROOT})");
    assert_eq!(session.value.cache.receipt, receipt);
    assert!(!database.exists(), "the refused database is discarded");
    assert!(!session.value.tags.cached());
    assert_eq!(session.diagnostics, Vec::<String>::new());
}

// The read-only open of the WAL fixture creates tags.db-shm and tags.db-wal, and with
// showUntrackedFiles=all a status that ran after the open would count them too.
#[test]
fn a_database_inside_the_worktree_opens_after_the_status_that_counts_it() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("home", &[("app.py", "x = 1\n")]);
    sandbox.git(&root, &["config", "status.showUntrackedFiles", "all"]);
    let services = sandbox.services(sandbox.path());
    let database = foreign_database(&sandbox, &services, &root);
    let top = services.git.root(&root);
    let before = services.git.snapshot(&root, top.as_deref());

    let session = opened(&services, &root, false);

    assert!(!outside(&database, top.as_deref()));
    assert_eq!(before.dirty_count, Some(1));
    assert_eq!(session.value.snapshot, before);
    assert!(!database.exists(), "the refused database is discarded");
}

#[test]
fn a_database_outside_the_worktree_opens_while_the_snapshot_runs() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    sandbox.put("repo/new.py", "y = 2\n");
    let services = sandbox.services(sandbox.path());
    let database = foreign_database(&sandbox, &services, &root);
    let top = services.git.root(&root);
    let expected = services.git.snapshot(&root, top.as_deref());

    let session = opened(&services, &root, false);

    assert!(outside(&database, top.as_deref()));
    assert_eq!(expected.dirty_count, Some(1));
    assert_eq!(session.value.snapshot, expected);
    let receipt = format!("none (cache discarded: it was built for {FIXTURE_ROOT})");
    assert_eq!(session.value.cache.receipt, receipt);
}

#[test]
fn only_a_database_below_the_top_level_is_inside_the_worktree() {
    let top = Path::new("/work/repo");

    assert!(!outside(Path::new("/work/repo/.cache/tags.db"), Some(top)));
    assert!(outside(Path::new("/work/repo-cache/tags.db"), Some(top)));
    assert!(outside(Path::new("/home/u/.cache/tags.db"), Some(top)));
    assert!(!outside(Path::new("/home/u/.cache/tags.db"), None));
}

#[test]
fn a_directory_outside_git_has_its_note_and_no_head() {
    let sandbox = Sandbox::new();
    let plain = sandbox.mkdir("plain");
    let services = sandbox.services(sandbox.path());
    let repo = crate::resolve(&services, Some(&plain)).expect("a directory resolves");

    let opening = Session::open(&services, repo, false);

    let note = format!(
        "{} is not inside a git repository: HEAD and dirty count are unknown",
        plain.display()
    );
    assert_eq!(opening.note, note);
    let session = opening.session.expect("a session");
    assert_eq!(session.value.snapshot.head_sha, None);
    assert_eq!(session.value.snapshot.note, note);
}

#[test]
fn a_cache_open_that_fails_still_gives_the_snapshot_note() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("pkg/a.py", "x = 1\n")]);
    let mut services = sandbox.services(sandbox.path());
    services.cache_root = Err(io::HomeUnknown);
    let nested = root.join("pkg");
    let repo = Repo::load(&services, &nested).expect("the directory loads");

    let opening = Session::open(&services, repo, false);

    let note = format!(
        "{} is not the top of its git repository: HEAD and dirty count describe {}",
        nested.display(),
        root.display()
    );
    assert_eq!(opening.note, note);
    let refused = opening.session.expect_err("no cache root");
    assert_eq!(refused.to_string(), "Could not determine home directory.");
}

#[test]
fn without_a_database_the_receipt_says_none_and_the_counts_are_zero() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let services = sandbox.services(sandbox.path());

    let absent = opened(&services, &root, false);
    let bypassed = opened(&services, &root, true);

    assert_eq!(absent.value.cache.receipt, "none");
    let counts = absent.value.cache.counts.expect("zero counts");
    assert_eq!((counts.files, counts.tags), (0, 0));
    assert_eq!(bypassed.value.cache.receipt, "bypassed (--no-cache)");
}

#[test]
fn the_hook_form_discards_a_refused_database_too() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let services = sandbox.services(sandbox.path());
    let cache_root = services.cache_root.as_ref().expect("a cache root");
    let database = store::database_path(&cache_root.path, root.to_str().expect("a UTF-8 path"));
    let relative = database
        .strip_prefix(sandbox.path())
        .expect("a sandbox path");
    sandbox.put(relative.to_str().expect("a UTF-8 path"), "not a database");
    let repo = Repo::load(&services, &root).expect("the repository loads");

    let tags = Tags::open(&services, &repo).expect("a cache root");

    assert!(!tags.value.cached());
    assert!(!database.exists(), "the corrupt database is discarded");
}

#[test]
fn an_unknown_home_fails_at_the_cache_open() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let mut services = sandbox.services(sandbox.path());
    services.cache_root = Err(io::HomeUnknown);
    let repo = Repo::load(&services, &root).expect("the repository loads");

    let refused = Tags::open(&services, &repo).expect_err("no cache root");

    assert_eq!(refused.to_string(), "Could not determine home directory.");
}
