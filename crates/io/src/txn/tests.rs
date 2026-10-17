use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;

use signal_hook::consts::SIGINT;

use super::*;
use crate::files::flushed;
use crate::fixtures::{is_child, run_child, tree};
use crate::testing::Tree;

const APP: &str = "def add(left, right):\n    return left + right\n";
const UTIL: &str = "VERSION = \"1\"\n";

fn repo() -> Tree {
    let tree = tree();
    tree.write_file("app.py", APP).expect("app.py is written");
    tree.write_file("util.py", UTIL)
        .expect("util.py is written");
    tree
}

fn read(tree: &Tree, name: &str) -> String {
    fs::read_to_string(tree.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn names(tree: &Tree) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(tree.path())
        .expect("the tree lists")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn with_suffix(tree: &Tree, suffix: &str) -> Vec<String> {
    names(tree)
        .into_iter()
        .filter(|name| name.ends_with(suffix))
        .collect()
}

const NEW_PAIR: [(&str, &str); 2] = [("app.py", "new app\n"), ("util.py", "new util\n")];

// The disk steps, with one call made to fail. A count is the 1-based call that fails.
#[derive(Default)]
struct Faulty {
    stage_fails: Option<(&'static str, &'static str)>,
    // The errno of every flush of a staged file, after its bytes are written.
    flush_fails_with: Option<i32>,
    reserve_fails_at: Option<usize>,
    staged_move_fails_at: Option<usize>,
    restore_fails: bool,
    remove_fails: bool,
    reserves: Cell<usize>,
    staged_moves: Cell<usize>,
    interrupt_on_first_move: bool,
}

fn bump(count: &Cell<usize>) -> usize {
    count.set(count.get() + 1);
    count.get()
}

fn refusal(message: &str) -> io::Error {
    io::Error::other(message.to_owned())
}

impl Steps for Faulty {
    fn stage(&self, target: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        match self.stage_fails {
            Some((name, message)) if target.file_name() == Some(OsStr::new(name)) => {
                Err(refusal(message))
            }
            _ => match self.flush_fails_with {
                Some(code) => stage_file(target, bytes, |file, bytes, name| {
                    write_bytes(file, bytes, name)?;
                    flushed(Err(io::Error::from_raw_os_error(code)))
                }),
                None => Disk.stage(target, bytes),
            },
        }
    }

    fn reserve(&self, target: &Path) -> io::Result<PathBuf> {
        if Some(bump(&self.reserves)) == self.reserve_fails_at {
            return Err(refusal("read-only file system"));
        }
        Disk.reserve(target)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let source = from.to_string_lossy();
        if source.ends_with(STAGING_SUFFIX)
            && Some(bump(&self.staged_moves)) == self.staged_move_fails_at
        {
            return Err(refusal("simulated replace failure"));
        }
        if source.ends_with(BACKUP_SUFFIX) && self.restore_fails {
            return Err(refusal("simulated restore failure"));
        }
        if self.interrupt_on_first_move && self.staged_moves.get() == 0 {
            signal_hook::low_level::raise(SIGINT).expect("SIGINT is raised");
        }
        Disk.rename(from, to)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        if self.remove_fails {
            return Err(refusal("read-only file system"));
        }
        Disk.remove(path)
    }
}

fn failed(steps: &Faulty, tree: &Tree, contents: &[(&str, &str)]) -> WriteFailure {
    write_with(steps, tree.path(), contents).expect_err("the write fails")
}

#[test]
fn every_file_is_written_and_no_sibling_stays() {
    let tree = repo();
    let leftovers = write_all(tree.path(), &NEW_PAIR).expect("the write succeeds");
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        ("new app\n".into(), "new util\n".into())
    );
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

#[test]
fn the_bytes_are_written_as_given() {
    let tree = repo();
    let crlf = "VERSION = \"1\"\r\nBUILD = 8\r\n";
    write_all(tree.path(), &[("util.py", crlf)]).expect("the write succeeds");
    assert_eq!(
        fs::read(tree.join("util.py")).expect("util.py reads"),
        crlf.as_bytes()
    );
}

#[test]
fn the_target_keeps_its_mode() {
    let tree = repo();
    tree.chmod("app.py", 0o751);
    write_all(tree.path(), &[("app.py", "new\n")]).expect("the write succeeds");
    let mode = fs::metadata(tree.join("app.py"))
        .expect("app.py exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o751);
}

#[test]
fn a_later_pair_for_a_path_replaces_an_earlier_one() {
    let tree = repo();
    write_all(
        tree.path(),
        &[("app.py", "first\n"), ("app.py", "second\n")],
    )
    .expect("the write succeeds");
    assert_eq!(read(&tree, "app.py"), "second\n");
}

#[test]
fn a_symlinked_target_writes_the_file_it_names_and_the_link_stays() {
    let tree = repo();
    tree.symlink("link.py", "app.py")
        .expect("the link is created");
    write_all(tree.path(), &[("link.py", "through the link\n")]).expect("the write succeeds");
    assert_eq!(read(&tree, "app.py"), "through the link\n");
    assert!(
        fs::symlink_metadata(tree.join("link.py"))
            .expect("the link exists")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn an_existing_staging_sibling_is_not_touched() {
    let tree = repo();
    tree.write_file(format!("app.py{STAGING_SUFFIX}"), "user-owned\n")
        .expect("the sibling is written");
    write_all(tree.path(), &[("app.py", "new\n")]).expect("the write succeeds");
    assert_eq!(
        read(&tree, &format!("app.py{STAGING_SUFFIX}")),
        "user-owned\n"
    );
}

#[test]
fn an_existing_staging_symlink_is_not_followed() {
    let tree = repo();
    let outside = tree
        .write_file("outside.txt", "outside\n")
        .expect("the file is written");
    tree.symlink(format!("app.py{STAGING_SUFFIX}"), &outside)
        .expect("the link is created");
    write_all(tree.path(), &[("app.py", "new\n")]).expect("the write succeeds");
    assert_eq!(read(&tree, "outside.txt"), "outside\n");
    let link = fs::symlink_metadata(tree.join(format!("app.py{STAGING_SUFFIX}")));
    assert!(link.expect("the link exists").file_type().is_symlink());
}

#[test]
fn a_stage_failure_names_the_path_and_changes_nothing() {
    let tree = repo();
    let steps = Faulty {
        stage_fails: Some(("util.py", "no space left on device")),
        ..Faulty::default()
    };
    let failure = failed(&steps, &tree, &NEW_PAIR);
    assert_eq!(
        failure.error.to_string(),
        "patch not applied: cannot write util.py: no space left on device; nothing was changed"
    );
    assert!(failure.leftovers.is_empty());
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        (APP.into(), UTIL.into())
    );
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

#[test]
fn a_flush_that_reports_a_deferred_error_is_a_stage_failure_and_changes_nothing() {
    let tree = repo();
    let steps = Faulty {
        flush_fails_with: Some(libc::EDQUOT),
        ..Faulty::default()
    };
    let failure = failed(&steps, &tree, &NEW_PAIR);
    assert_eq!(
        failure.error.to_string(),
        "patch not applied: cannot write app.py: Disk quota exceeded; nothing was changed"
    );
    assert!(failure.leftovers.is_empty());
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        (APP.into(), UTIL.into())
    );
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

#[test]
fn a_file_system_with_no_flush_still_takes_the_write() {
    let tree = repo();
    let steps = Faulty {
        flush_fails_with: Some(libc::EINVAL),
        ..Faulty::default()
    };
    write_with(&steps, tree.path(), &NEW_PAIR).expect("the write succeeds");
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        ("new app\n".into(), "new util\n".into())
    );
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

#[test]
fn a_staging_write_that_fails_leaves_no_staging_file() {
    let tree = repo();
    let refuse = |_: &mut File, _: &[u8], _: &OsStr| Err(refusal("no space left on device"));
    let error =
        stage_file(&tree.join("app.py"), b"new app\n", refuse).expect_err("the stage fails");
    assert_eq!(strerror(&error), "no space left on device");
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

#[test]
fn a_stage_for_a_missing_target_leaves_no_staging_file() {
    let tree = repo();
    let error = Disk
        .stage(&tree.join("gone.py"), b"x")
        .expect_err("the target is missing");
    assert_eq!(strerror(&error), "No such file or directory");
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

struct ZeroWriter;

impl Write for ZeroWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_write_that_makes_no_progress_is_an_error() {
    let error =
        write_bytes(&mut ZeroWriter, b"content", OsStr::new("app.py")).expect_err("no progress");
    assert_eq!(strerror(&error), "write returned zero bytes for app.py");
    write_bytes(&mut ZeroWriter, b"", OsStr::new("app.py")).expect("nothing to write is no error");
}

#[test]
fn a_backup_that_cannot_be_reserved_changes_nothing() {
    for at in [1, 2] {
        let tree = repo();
        let steps = Faulty {
            reserve_fails_at: Some(at),
            ..Faulty::default()
        };
        let failure = failed(&steps, &tree, &NEW_PAIR);
        assert_eq!(
            failure.error.to_string(),
            "patch not applied: cannot reserve rollback files: read-only file system; nothing changed"
        );
        assert_eq!(
            (read(&tree, "app.py"), read(&tree, "util.py")),
            (APP.into(), UTIL.into())
        );
        assert_eq!(names(&tree), ["app.py", "util.py"], "reserve {at}");
    }
}

#[test]
fn a_replace_failure_restores_every_original() {
    let tree = repo();
    let steps = Faulty {
        staged_move_fails_at: Some(2),
        ..Faulty::default()
    };
    let failure = failed(&steps, &tree, &NEW_PAIR);
    assert_eq!(
        failure.error.to_string(),
        "patch not applied: cannot replace util.py: simulated replace failure; every original was restored"
    );
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        (APP.into(), UTIL.into())
    );
    assert_eq!(names(&tree), ["app.py", "util.py"]);
}

#[test]
fn a_failure_at_the_third_of_five_leaves_no_residue() {
    let tree = tree();
    let originals: Vec<(String, String)> = (0..5)
        .map(|index| (format!("m{index}.py"), format!("VALUE = {index}\n")))
        .collect();
    for (name, content) in &originals {
        tree.write_file(name, content)
            .expect("the original is written");
    }
    let rewritten: Vec<(String, String)> = originals
        .iter()
        .enumerate()
        .map(|(index, (name, _))| (name.clone(), format!("REWRITTEN = {index}\n")))
        .collect();
    let steps = Faulty {
        staged_move_fails_at: Some(3),
        ..Faulty::default()
    };
    let failure = write_with(&steps, tree.path(), &rewritten).expect_err("the write fails");
    assert!(
        failure
            .error
            .to_string()
            .ends_with("every original was restored"),
        "{}",
        failure.error
    );
    let expected: Vec<String> = originals.iter().map(|(name, _)| name.clone()).collect();
    assert_eq!(names(&tree), expected);
    for (name, content) in &originals {
        assert_eq!(&read(&tree, name), content);
    }
}

#[test]
fn a_rollback_that_also_fails_names_every_surviving_backup() {
    let tree = repo();
    let steps = Faulty {
        staged_move_fails_at: Some(2),
        restore_fails: true,
        ..Faulty::default()
    };
    let failure = failed(&steps, &tree, &NEW_PAIR);
    let message = failure.error.to_string();
    let WriteError::PartlyApplied { retained, .. } = &failure.error else {
        panic!("not a partial apply: {message}");
    };
    let shown: Vec<String> = retained
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    assert_eq!(
        message,
        format!(
            "patch partly applied: cannot replace util.py: simulated replace failure; rollback \
             failed; originals retained at: {}",
            shown.join(", ")
        )
    );
    let backups = with_suffix(&tree, BACKUP_SUFFIX);
    let original = |name: &String| name.split('.').nth(1).map(str::to_owned);
    let mut originals: Vec<_> = backups.iter().filter_map(original).collect();
    originals.sort();
    assert_eq!(originals, ["app", "util"]);
    assert!(
        !tree.join("util.py").exists(),
        "util.py is only its backup now"
    );
    assert_eq!(read(&tree, "app.py"), "new app\n");
    // The restore runs last first, so the message names util.py's backup first.
    let first = retained[0]
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    assert!(first.starts_with(".util.py."), "{first}");
    assert_eq!(
        fs::read_to_string(&retained[0]).expect("the backup reads"),
        UTIL
    );
    assert_eq!(
        fs::read_to_string(&retained[1]).expect("the backup reads"),
        APP
    );
    assert!(with_suffix(&tree, STAGING_SUFFIX).is_empty());
}

#[test]
fn a_sibling_that_cannot_be_removed_is_a_leftover() {
    let tree = repo();
    let steps = Faulty {
        remove_fails: true,
        ..Faulty::default()
    };
    let leftovers = write_with(&steps, tree.path(), &NEW_PAIR).expect("the write succeeds");
    assert_eq!(leftovers.len(), 2);
    let shown = leftovers[0].to_string();
    assert_eq!(
        shown,
        format!(
            "could not remove the temporary file {}: read-only file system; delete it by hand",
            leftovers[0].path.display()
        )
    );
    assert!(
        leftovers[0].path.to_string_lossy().ends_with(BACKUP_SUFFIX),
        "{shown}"
    );
}

#[test]
fn a_failed_stage_names_the_staged_files_it_cannot_remove() {
    let tree = repo();
    let steps = Faulty {
        stage_fails: Some(("util.py", "no space left on device")),
        remove_fails: true,
        ..Faulty::default()
    };
    let failure = failed(&steps, &tree, &NEW_PAIR);
    assert_eq!(failure.leftovers.len(), 1);
    let left = failure.leftovers[0]
        .path
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    assert!(
        left.starts_with(".app.py.") && left.ends_with(STAGING_SUFFIX),
        "{left}"
    );
}

#[test]
fn a_path_outside_the_base_is_refused_and_the_staged_files_are_removed() {
    let tree = repo();
    let outside = tree
        .create_dir("outside")
        .expect("the directory is created");
    let base = tree.create_dir("base").expect("the directory is created");
    tree.write_file("base/a.py", "a\n")
        .expect("a.py is written");
    tree.symlink("base/b.py", outside.join("b.py"))
        .expect("the link is created");
    let failure = write_all(&base, &[("a.py", "new\n"), ("b.py", "new\n")]).expect_err("refused");
    assert!(
        matches!(failure.error, WriteError::Refused(_)),
        "{:?}",
        failure.error
    );
    assert!(
        failure
            .error
            .to_string()
            .starts_with("path refused: resolved to "),
        "{}",
        failure.error
    );
    let left: Vec<_> = fs::read_dir(&base)
        .expect("base lists")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
    assert_eq!(
        fs::read_to_string(base.join("a.py")).expect("a.py reads"),
        "a\n"
    );
    assert!(matches!(
        kernel::Error::from(failure.error),
        kernel::Error::SecurityRefusal(_)
    ));
}

#[test]
fn a_watch_that_cannot_register_changes_nothing() {
    let error = watch_error(&io::Error::from_raw_os_error(22));
    assert_eq!(
        error.to_string(),
        "patch not applied: cannot hold SIGINT: Invalid argument; nothing was changed"
    );
}

#[test]
fn a_read_only_directory_is_a_stage_failure_with_the_os_message() {
    let tree = repo();
    if !tree.modes_bind() {
        return;
    }
    tree.chmod(".", 0o555);
    let failure = write_all(tree.path(), &NEW_PAIR).expect_err("the directory refuses the write");
    tree.chmod(".", 0o755);
    assert_eq!(
        failure.error.to_string(),
        "patch not applied: cannot write app.py: Permission denied; nothing was changed"
    );
    assert!(matches!(
        kernel::Error::from(failure.error),
        kernel::Error::OperationFailed(_)
    ));
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        (APP.into(), UTIL.into())
    );
}

#[test]
fn a_sigint_during_the_moves_is_held_until_the_write_is_whole() {
    let name = "txn::tests::child_raises_sigint_during_the_moves";
    let (status, stderr) = run_child(name);
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("the write was whole"), "{stderr}");
}

#[test]
fn child_raises_sigint_during_the_moves() {
    if !is_child("txn::tests::child_raises_sigint_during_the_moves") {
        return;
    }
    let tree = repo();
    let steps = Faulty {
        interrupt_on_first_move: true,
        ..Faulty::default()
    };
    let written = write_held(&steps, tree.path(), &NEW_PAIR);
    assert!(written.is_ok(), "{:?}", written.err());
    assert!(crate::interrupted(), "the SIGINT was not recorded");
    assert_eq!(
        (read(&tree, "app.py"), read(&tree, "util.py")),
        ("new app\n".into(), "new util\n".into())
    );
    assert_eq!(names(&tree), ["app.py", "util.py"]);
    eprintln!("the write was whole");
}

#[test]
fn a_sigint_with_no_hold_ends_the_process() {
    let (status, stderr) = run_child("txn::tests::child_raises_sigint_with_no_hold");
    assert_eq!(status.signal(), Some(SIGINT), "{status:?} {stderr}");
}

#[test]
fn child_raises_sigint_with_no_hold() {
    if !is_child("txn::tests::child_raises_sigint_with_no_hold") {
        return;
    }
    drop(hold_sigint().expect("the watch registers"));
    signal_hook::low_level::raise(SIGINT).expect("SIGINT is raised");
    eprintln!("the process outlived SIGINT");
}
