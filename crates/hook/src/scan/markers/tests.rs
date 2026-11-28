use std::fs;
use std::path::{Path, PathBuf};

use super::{CAP, Markers};
use crate::testing::Sandbox;

fn markers(sandbox: &Sandbox, session: &str) -> Markers {
    Markers::new(&sandbox.path().join("state"), session)
}

#[test]
fn a_marked_root_is_listed_with_its_path_for_its_session_only() {
    let sandbox = Sandbox::new();
    let mine = markers(&sandbox, "one");
    let other = markers(&sandbox, "two");

    assert!(mine.mark(Path::new("/work/b")));
    assert!(mine.mark(Path::new("/work/a")));
    assert!(mine.mark(Path::new("/work/a")));

    let roots: Vec<PathBuf> = vec!["/work/a".into(), "/work/b".into()];
    assert_eq!(mine.marked(), roots);
    assert_eq!(other.marked(), Vec::<PathBuf>::new());
}

#[test]
fn the_marker_names_are_digests_and_hold_the_root_path() {
    let sandbox = Sandbox::new();
    let held = markers(&sandbox, "one");
    held.mark(Path::new("/work/a"));

    let session = fs::read_dir(sandbox.path().join("state")).expect("the state folder");
    let session = session.map(|entry| entry.expect("an entry").path()).next();
    let repos = session.expect("one session folder").join("repos");
    let files: Vec<PathBuf> = fs::read_dir(repos)
        .expect("the repos folder")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    let [file] = files.as_slice() else {
        panic!("one marker: {files:?}");
    };
    let name = file.file_name().expect("a name").to_string_lossy();
    assert_eq!(name.len(), 64);
    assert!(name.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(fs::read_to_string(file).expect("the content"), "/work/a");
}

#[test]
fn a_finding_is_a_first_report_once_for_its_path_and_key() {
    let sandbox = Sandbox::new();
    let held = markers(&sandbox, "one");

    assert!(held.first_report("/r/a.py", "run\0x"));
    assert!(!held.first_report("/r/a.py", "run\0x"));
    assert!(held.first_report("/r/a.py", "run\0y"));
    assert!(held.first_report("/r/b.py", "run\0x"));
    assert!(markers(&sandbox, "two").first_report("/r/a.py", "run\0x"));
}

#[test]
fn a_full_reported_folder_reports_with_no_deduplication() {
    let sandbox = Sandbox::new();
    let held = markers(&sandbox, "one");
    assert!(held.first_report("/r/a.py", "first"));
    let folder = fs::read_dir(sandbox.path().join("state"))
        .expect("the state folder")
        .map(|entry| entry.expect("an entry").path().join("reported"))
        .next()
        .expect("one session folder");
    for index in 0..CAP {
        let name = format!("filler{index}");
        if fs::read_dir(&folder).expect("the folder").count() >= CAP {
            break;
        }
        io::write_file(&folder.join(name), b"").expect("a filler");
    }

    assert!(held.first_report("/r/new.py", "fresh"));
    assert!(held.first_report("/r/new.py", "fresh"));
    assert!(!held.first_report("/r/a.py", "first"));
}

#[test]
fn a_full_repos_folder_keeps_no_new_mark() {
    let sandbox = Sandbox::new();
    let held = markers(&sandbox, "one");
    assert!(held.mark(Path::new("/work/a")));
    let folder = fs::read_dir(sandbox.path().join("state"))
        .expect("the state folder")
        .map(|entry| entry.expect("an entry").path().join("repos"))
        .next()
        .expect("one session folder");
    for index in 0..CAP {
        if fs::read_dir(&folder).expect("the folder").count() >= CAP {
            break;
        }
        io::write_file(&folder.join(format!("filler{index}")), b"").expect("a filler");
    }

    assert!(!held.mark(Path::new("/work/new")));
    assert!(held.mark(Path::new("/work/a")));
}

#[test]
fn a_state_folder_that_others_can_write_lists_no_root() {
    let sandbox = Sandbox::new();
    let held = markers(&sandbox, "one");
    assert!(held.mark(Path::new("/work/a")));
    sandbox.set_mode("state", 0o777);

    assert_eq!(held.marked(), Vec::<PathBuf>::new());
    sandbox.set_mode("state", 0o700);
    assert_eq!(held.marked(), [PathBuf::from("/work/a")]);
}

#[test]
fn a_marker_that_is_no_absolute_path_is_not_a_root() {
    let sandbox = Sandbox::new();
    let held = markers(&sandbox, "one");
    held.mark(Path::new("/work/a"));
    let folder = fs::read_dir(sandbox.path().join("state"))
        .expect("the state folder")
        .map(|entry| entry.expect("an entry").path().join("repos"))
        .next()
        .expect("one session folder");
    io::write_file(&folder.join("relative"), b"not/absolute").expect("a marker");

    assert_eq!(held.marked(), [PathBuf::from("/work/a")]);
}
