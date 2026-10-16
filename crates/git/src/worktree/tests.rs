use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use super::*;
use crate::testing::{Scratch, path_before};
use crate::{Failure, Unfinished};

const APP: &str = "def add(a, b):\n    return a + b\n";
const FILES: [(&str, &str); 2] = [("app.py", APP), ("README.md", "# fixture\n")];

fn repo(scratch: &Scratch) -> PathBuf {
    scratch.repo("repo", &FILES)
}

fn cache(scratch: &Scratch) -> PathBuf {
    scratch.root.join("cache")
}

fn text(scratch: &Scratch, dir: &Path, args: &[&str]) -> String {
    String::from_utf8(scratch.run_git(dir, args)).expect("git prints UTF-8 here")
}

fn head(scratch: &Scratch, root: &Path) -> String {
    text(scratch, root, &["rev-parse", "HEAD"])
        .trim_end()
        .to_owned()
}

fn created(scratch: &Scratch, root: &Path) -> Worktree {
    let commit = head(scratch, root);
    let made = scratch.git().worktree(root, &commit, &cache(scratch));
    made.unwrap_or_else(|failure| panic!("{}", failure.error))
}

fn refused(scratch: &Scratch, root: &Path, cache_root: &Path) -> WorktreeFailure {
    scratch
        .git()
        .worktree(root, "HEAD", cache_root)
        .expect_err("the worktree is refused")
}

// The entries of a directory; a directory that is not there has none.
fn entries(directory: &Path) -> Vec<PathBuf> {
    match fs::read_dir(directory) {
        Ok(listed) => listed
            .map(|entry| entry.expect("an entry").path())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("{}: {error}", directory.display()),
    }
}

fn scratch_entries(scratch: &Scratch) -> Vec<PathBuf> {
    entries(&cache(scratch).join(WORKTREE_DIR))
}

fn write_in(tree: &Worktree, text: &str) {
    let written = io::write_file(&tree.inside().join("app.py"), text.as_bytes());
    written.expect("the worktree takes the write");
}

const CHANGED: &str = "def add(a, b):\n    return a - b\n";

#[test]
fn a_worktree_holds_the_commit_and_lives_under_the_cache_root() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let tree = created(&scratch, &root);
    let app = fs::read_to_string(tree.inside().join("app.py")).expect("app.py reads");
    assert_eq!(app, APP);
    assert!(!tree.inside().starts_with(&root));
    let scratch_dir = cache(&scratch).join(WORKTREE_DIR);
    assert_eq!(tree.path().parent(), Some(scratch_dir.as_path()));
    let name = tree
        .path()
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    let hex = name.strip_prefix("wt-").expect("the wt- prefix");
    assert!(
        hex.len() == 32 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{name}"
    );
    let mode = fs::metadata(&scratch_dir)
        .expect("the scratch exists")
        .permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o700
    );
    assert_eq!(tree.release(), []);
}

#[test]
fn a_release_removes_the_scratch_and_the_record() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let tree = created(&scratch, &root);
    let path = tree.path().to_path_buf();
    assert_eq!(tree.release(), []);
    assert!(!path.exists());
    let listed = text(&scratch, &root, &["worktree", "list"]);
    assert!(!listed.contains(&*path.to_string_lossy()), "{listed}");
}

#[test]
fn the_checkout_is_unchanged_by_a_write_in_the_worktree() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let state = || {
        (
            text(&scratch, &root, &["status", "--porcelain"]),
            head(&scratch, &root),
        )
    };
    let before = state();
    let tree = created(&scratch, &root);
    write_in(&tree, CHANGED);
    assert_eq!(tree.release(), []);
    assert_eq!(state(), before);
    assert_eq!(
        fs::read_to_string(root.join("app.py")).expect("app.py reads"),
        APP
    );
}

#[test]
fn a_panic_inside_a_worktree_still_removes_it() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let seen: Mutex<Option<PathBuf>> = Mutex::new(None);
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let tree = created(&scratch, &root);
        *seen.lock().expect("the slot locks") = Some(tree.path().to_path_buf());
        panic!("deliberate");
    }));
    assert!(outcome.is_err());
    let path = seen.into_inner().expect("the slot unlocks");
    let path = path.expect("the worktree was made");
    assert!(!path.exists(), "the drop left {}", path.display());
    assert_eq!(scratch_entries(&scratch), Vec::<PathBuf>::new());
}

#[test]
fn a_poisoned_lock_does_not_stop_the_next_worktree() {
    let poisoned = thread::spawn(|| {
        let _held = lock();
        panic!("a holder of the worktree lock panics");
    })
    .join();
    assert!(poisoned.is_err());
    let scratch = Scratch::new();
    let root = repo(&scratch);
    assert_eq!(created(&scratch, &root).release(), []);
}

#[test]
fn a_subdirectory_root_gives_that_subdirectory() {
    let scratch = Scratch::new();
    let root = scratch.repo("mono", &[("pkg/app.py", APP), ("README.md", "# top\n")]);
    let commit = head(&scratch, &root);
    let made = scratch
        .git()
        .worktree(&root.join("pkg"), &commit, &cache(&scratch));
    let tree = made.unwrap_or_else(|failure| panic!("{}", failure.error));
    assert_eq!(tree.inside(), tree.path().join("pkg"));
    let app = fs::read_to_string(tree.inside().join("app.py")).expect("app.py reads");
    assert_eq!(app, APP);
    assert_eq!(tree.release(), []);
}

#[test]
fn two_worktrees_do_not_collide() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let first = created(&scratch, &root);
    let second = created(&scratch, &root);
    assert_ne!(first.path(), second.path());
    assert_eq!((first.release(), second.release()), (vec![], vec![]));
}

#[test]
fn a_directory_outside_git_is_refused() {
    let scratch = Scratch::new();
    let plain = scratch.create_dir("plain");
    let failure = refused(&scratch, &plain, &cache(&scratch));
    assert_eq!(
        failure.error.to_string(),
        format!(
            "{} is not inside a git repository, so no worktree can be created for it. Patch \
             apply needs a git repository; run it against one, or use --in-place.",
            plain.display()
        )
    );
    assert_eq!(failure.cleanup, []);
    let mapped = kernel::Error::from(failure.error);
    assert!(matches!(mapped, kernel::Error::RepoResolution(_)));
}

#[test]
fn a_scratch_inside_the_repository_is_refused_and_not_created() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let failure = refused(&scratch, &root, &root.join("inside"));
    let expected = format!(
        "scratch worktrees would be created at {}, inside the repository {} they are meant to \
         stay out of. Point XDG_CACHE_HOME at a directory outside the repository.",
        root.join("inside").join(WORKTREE_DIR).display(),
        root.display()
    );
    assert_eq!(failure.error.to_string(), expected);
    assert!(
        !root.join("inside").exists(),
        "the refused location was created anyway"
    );
}

#[test]
fn a_scratch_symlinked_into_the_repository_is_refused() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let inside = scratch.create_dir("repo/inside");
    scratch.symlink("cache-link", &inside);
    let failure = refused(&scratch, &root, &scratch.root.join("cache-link"));
    let refused_inside = matches!(failure.error, WorktreeError::ScratchInside { .. });
    assert!(refused_inside, "{}", failure.error);
    assert_eq!(entries(&inside), Vec::<PathBuf>::new());
}

#[test]
fn a_root_that_the_commit_lacks_is_named_and_the_worktree_is_released() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let untracked = scratch.create_dir("repo/scratchpad");
    let commit = head(&scratch, &root);
    let made = scratch
        .git()
        .worktree(&untracked, &commit, &cache(&scratch));
    let failure = made.expect_err("the commit has no scratchpad");
    assert_eq!(
        failure.error.to_string(),
        format!(
            "{} is not in commit {commit} of {}, so the worktree has no scratchpad to work in. \
             The directory is untracked or ignored: commit it, or point this call at a \
             directory the repository tracks.",
            untracked.display(),
            root.display()
        )
    );
    assert_eq!(failure.cleanup, []);
    assert_eq!(scratch_entries(&scratch), Vec::<PathBuf>::new());
}

#[test]
fn a_short_id_expands_to_the_full_commit_and_a_later_commit_does_not_move_it() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let full = head(&scratch, &root);
    scratch.write("repo/app.py", "def add(a, b):\n    return 0\n");
    scratch.commit_all(&root, "moved");
    let pinned = scratch.git().pinned_commit(&root, Some(&full[..8]));
    let pinned = pinned.expect("the pin resolves");
    assert_eq!(pinned, full);
    let made = scratch.git().worktree(&root, &pinned, &cache(&scratch));
    let tree = made.unwrap_or_else(|failure| panic!("{}", failure.error));
    let app = fs::read_to_string(tree.inside().join("app.py")).expect("app.py reads");
    assert_eq!(app, APP);
    assert_eq!(tree.release(), []);
}

#[test]
fn no_captured_commit_is_refused() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    for captured in [None, Some("")] {
        let error = scratch
            .git()
            .pinned_commit(&root, captured)
            .expect_err("no commit");
        assert_eq!(
            error.to_string(),
            format!(
                "{} is not inside a git repository, or HEAD names no commit, so no worktree can \
                 be created for it.",
                root.display()
            )
        );
        assert!(matches!(
            kernel::Error::from(error),
            kernel::Error::RepoResolution(_)
        ));
    }
}

#[test]
fn a_failing_git_call_names_the_directory() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let pinned = scratch.git().pinned_commit(&root, Some("no-such-branch"));
    let error = pinned.expect_err("the revision is unknown");
    let shown = error.to_string();
    assert!(shown.starts_with("git rev-parse exited 128: "), "{shown}");
    assert!(
        shown.ends_with(&format!(" (in {})", root.display())),
        "{shown}"
    );
    assert!(matches!(
        kernel::Error::from(error),
        kernel::Error::OperationFailed(_)
    ));
}

#[test]
fn a_missing_git_says_git_is_not_installed() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let empty = scratch.create_dir("empty-bin");
    let git = Git::new(scratch.env_with(&[("PATH", empty.as_os_str())]));
    let error = git.worktree_diff(&root).expect_err("git is missing");
    let expected = "git is not installed, so repository state is unknown";
    assert_eq!(
        error.to_string(),
        format!("{expected} (in {})", root.display())
    );
}

#[test]
fn each_runner_answer_maps_to_its_error() {
    let cwd = Path::new("/repo");
    let timed_out = Unfinished::TimedOut {
        subcommand: "status",
        after: "7.0",
    };
    let not_run = Unfinished::NotRun {
        subcommand: "status",
        reason: "Too many open files".to_owned(),
    };
    let failed = Failure {
        subcommand: "worktree",
        code: 128,
        detail: "fatal: refused".to_owned(),
    };
    let shown = |outcome| checked("status", cwd, outcome).map_err(|error| error.to_string());
    assert_eq!(shown(GitOutcome::Printed(b"x".to_vec())), Ok(b"x".to_vec()));
    let cases = [
        (
            GitOutcome::Capped(b"x".to_vec()),
            "git status printed more than 8000000 bytes in /repo",
        ),
        (
            GitOutcome::Unfinished(timed_out),
            "git status timed out after 7.0s (in /repo)",
        ),
        (
            GitOutcome::Unfinished(not_run),
            "git status could not be run: Too many open files (in /repo)",
        ),
        (
            GitOutcome::Failed(failed),
            "git worktree exited 128: fatal: refused (in /repo)",
        ),
    ];
    for (outcome, expected) in cases {
        assert_eq!(shown(outcome), Err(expected.to_owned()));
    }
}

#[test]
fn the_cleanup_bound_is_shorter_than_the_creation_bound() {
    assert_eq!((CREATE.seconds, CLEANUP.seconds), (120, 30));
    assert_eq!((CREATE.cap, CLEANUP.cap), (8_000_000, 8_000_000));
}

#[test]
fn a_written_change_shows_as_a_unified_diff_and_an_untouched_tree_as_nothing() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let tree = created(&scratch, &root);
    assert_eq!(
        scratch
            .git()
            .worktree_diff(tree.inside())
            .expect("the diff runs"),
        ""
    );
    write_in(&tree, CHANGED);
    let diff = scratch
        .git()
        .worktree_diff(tree.inside())
        .expect("the diff runs");
    for line in [
        "--- a/app.py",
        "+++ b/app.py",
        "-    return a + b",
        "+    return a - b",
    ] {
        assert!(diff.contains(line), "{diff}");
    }
    assert_eq!(tree.release(), []);
}

// A hook that writes `marker` when a checkout fires it.
fn install_hook(scratch: &Scratch, directory: &str, marker: &Path) {
    let relative = format!("{directory}/post-checkout");
    let body = format!("#!/bin/sh\necho fired > \"{}\"\n", marker.display());
    scratch.write(&relative, &body);
    scratch.set_mode(&relative, 0o755);
}

#[test]
fn a_hook_that_a_plain_checkout_runs_is_not_run() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let marker = scratch.root.join("hook-fired.txt");
    install_hook(&scratch, "repo/.git/hooks", &marker);
    let control = scratch.root.join("control-worktree");
    let control = control.to_str().expect("a UTF-8 path");
    scratch.run_git(&root, &["worktree", "add", "--detach", control, "HEAD"]);
    assert!(
        marker.exists(),
        "the fixture hook never fired; the test proves nothing"
    );
    scratch.run_git(&root, &["worktree", "remove", "--force", control]);
    io::remove_file(&marker).expect("the marker is removed");

    assert_eq!(created(&scratch, &root).release(), []);
    assert!(
        !marker.exists(),
        "git worktree add ran the repository's post-checkout hook"
    );
}

#[test]
fn a_configured_hooks_path_is_neutralised_too() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let marker = scratch.root.join("hook-fired.txt");
    install_hook(&scratch, "repo/githooks", &marker);
    scratch.run_git(&root, &["config", "core.hooksPath", "githooks"]);
    assert_eq!(created(&scratch, &root).release(), []);
    assert!(
        !marker.exists(),
        "core.hooksPath from the repository decided what ran"
    );
}

#[test]
fn a_textconv_driver_that_the_repository_names_is_not_run() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let marker = scratch.root.join("textconv-fired.txt");
    let driver = format!(
        "#!/bin/sh\necho fired > \"{}\"\ncat \"$1\"\n",
        marker.display()
    );
    scratch.write("repo/textconv.sh", &driver);
    scratch.set_mode("repo/textconv.sh", 0o755);
    scratch.write("repo/.gitattributes", "*.py diff=shipped\n");
    scratch.commit_all(&root, "ship a diff driver");
    let driver = root.join("textconv.sh");
    let driver = driver.to_str().expect("a UTF-8 path");
    scratch.run_git(&root, &["config", "diff.shipped.textconv", driver]);
    let tree = created(&scratch, &root);
    write_in(&tree, CHANGED);
    scratch.run_git(tree.inside(), &["diff", "--no-color", "--no-ext-diff"]);
    assert!(
        marker.exists(),
        "the fixture driver never fired; the test proves nothing"
    );
    io::remove_file(&marker).expect("the marker is removed");

    let diff = scratch
        .git()
        .worktree_diff(tree.inside())
        .expect("the diff runs");
    assert!(diff.contains("return a - b"), "{diff}");
    assert!(!marker.exists(), "the repository's textconv driver ran");
    assert_eq!(tree.release(), []);
}

fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").expect("the tests find git through PATH");
    std::env::split_paths(&path)
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git is on PATH")
}

fn faked(scratch: &Scratch, body: &str) -> Git {
    let bin = scratch.fake_git(body, 0o755);
    Git::new(scratch.env_with(&[("PATH", path_before(bin).as_os_str())]))
}

// A git that runs the real one without its stderr, then fails as a kill would after `worktree
// add` made its record.
fn killed_add_git(scratch: &Scratch, root: &Path) -> Git {
    let body = format!(
        "#!/bin/sh\n\"{real}\" \"$@\" 2>/dev/null || exit $?\ncase \" $* \" in\n*\" worktree add \"*)\n  \
         for record in \"{root}\"/.git/worktrees/*; do echo killed > \"$record/locked\"; done\n  \
         echo 'fatal: simulated kill' >&2\n  exit 128;;\nesac\n",
        real = real_git().display(),
        root = root.display()
    );
    faked(scratch, &body)
}

#[test]
fn a_killed_creation_leaves_no_locked_record_and_no_scratch() {
    let scratch = Scratch::new();
    let root = repo(&scratch);
    let git = killed_add_git(&scratch, &root);
    let commit = head(&scratch, &root);
    let made = git.worktree(&root, &commit, &cache(&scratch));
    let failure = made.expect_err("the add was killed");
    let expected = format!(
        "git worktree exited 128: fatal: simulated kill (in {})",
        root.display()
    );
    assert_eq!(failure.error.to_string(), expected);
    assert_eq!(failure.cleanup, []);
    assert!(matches!(
        kernel::Error::from(failure.error),
        kernel::Error::OperationFailed(_)
    ));
    assert_eq!(entries(&root.join(".git/worktrees")), Vec::<PathBuf>::new());
    assert!(!text(&scratch, &root, &["worktree", "list"]).contains("wt-"));
    assert_eq!(scratch_entries(&scratch), Vec::<PathBuf>::new());
}

// A git that refuses the named worktree subcommand and succeeds at every other call.
fn refusing_git(scratch: &Scratch, refused: &str) -> Git {
    let body = format!(
        "#!/bin/sh\ncase \" $* \" in\n*\" worktree {refused}\"*)\n  echo 'fatal: refused' >&2\n  \
         exit 128;;\nesac\nexit 0\n"
    );
    faked(scratch, &body)
}

fn fixture_tree(scratch: &Scratch, relative: &str) -> PathBuf {
    scratch.write(&format!("{relative}/app.py"), APP);
    scratch.root.join(relative)
}

fn refusal(scratch: &Scratch) -> RunGitError {
    RunGitError::Failed {
        note: "git worktree exited 128: fatal: refused".to_owned(),
        cwd: scratch.root.clone(),
    }
}

#[test]
fn a_failed_remove_falls_back_to_removing_the_directory() {
    let scratch = Scratch::new();
    let path = fixture_tree(&scratch, "wt-fixture");
    let notes = release(&refusing_git(&scratch, "remove"), &scratch.root, &path);
    assert!(!path.exists());
    let error = refusal(&scratch);
    assert_eq!(
        notes,
        [CleanupNote::RemoveFailed {
            path: path.clone(),
            error
        }]
    );
    assert_eq!(
        notes[0].to_string(),
        format!(
            "git worktree remove failed for {}: git worktree exited 128: fatal: refused (in {}); \
             removing the directory",
            path.display(),
            scratch.root.display()
        )
    );
}

#[test]
fn a_failed_prune_is_a_note() {
    let scratch = Scratch::new();
    let path = fixture_tree(&scratch, "wt-fixture");
    let notes = release(&refusing_git(&scratch, "prune"), &scratch.root, &path);
    let shown: Vec<String> = notes.iter().map(ToString::to_string).collect();
    let pruned = format!(
        "git worktree prune failed in {root}: git worktree exited 128: fatal: refused (in {root})",
        root = scratch.root.display()
    );
    // The fake git removes nothing, so the directory stays and its note follows.
    let leftover = format!(
        "scratch worktree {} could not be removed and is still on disk; delete it by hand",
        path.display()
    );
    assert_eq!(shown, [pruned, leftover]);
}

#[test]
fn a_directory_that_no_rung_removes_is_named_for_an_operator() {
    let scratch = Scratch::new();
    if !scratch.modes_bind() {
        return;
    }
    let path = fixture_tree(&scratch, "locked/wt-fixture");
    let git = refusing_git(&scratch, "remove");
    scratch.set_mode("locked", 0o555);
    let notes = release(&git, &scratch.root, &path);
    scratch.set_mode("locked", 0o755);
    assert!(path.exists());
    let error = refusal(&scratch);
    let expected = [
        CleanupNote::RemoveFailed {
            path: path.clone(),
            error,
        },
        CleanupNote::Leftover { path },
    ];
    assert_eq!(notes, expected);
}

#[test]
fn an_os_error_and_a_root_outside_its_top_have_their_texts() {
    let error = WorktreeError::Os {
        path: PathBuf::from("/cache/worktrees"),
        error: std::io::Error::from_raw_os_error(13),
    };
    let expected = "PermissionError: [Errno 13] Permission denied: '/cache/worktrees'";
    assert_eq!(error.to_string(), expected);
    let outside = WorktreeError::OutsideTop {
        resolved: PathBuf::from("/a/b"),
        top: PathBuf::from("/c"),
    };
    assert_eq!(outside.to_string(), "'/a/b' is not in the subpath of '/c'");
}
