use std::ffi::OsStr;
use std::path::Path;

use super::tree_files;
use crate::run::NO_LAZY_FETCH;
use crate::testing::{Scratch, path_before};
use crate::walk::{BOUNDS, Bounds, LISTING, WalkError};
use crate::{GitOutcome, RepoFile, Walk};

const EVERY: io::ScanReads = |_, _| true;

fn listed(walk: Walk) -> Vec<(String, u64)> {
    let files = walk.files.unwrap_or_else(|error| panic!("{error}"));
    files
        .into_iter()
        .map(|RepoFile { path, size }| (path, size))
        .collect()
}

fn head(scratch: &Scratch, root: &Path) -> String {
    let printed = scratch.run_git(root, &["rev-parse", "HEAD"]);
    String::from_utf8(printed)
        .expect("a hex id")
        .trim()
        .to_owned()
}

fn owned(rows: &[(&str, u64)]) -> Vec<(String, u64)> {
    rows.iter()
        .map(|(path, size)| ((*path).to_owned(), *size))
        .collect()
}

#[test]
fn the_walk_lists_the_files_of_the_commit_and_not_of_the_working_tree() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "x = 1\n"), ("pkg/b.py", "y = 22\n")]);
    let commit = head(&scratch, &root);
    scratch.write("repo/a.py", "x = 1000\n");
    scratch.run_git(&root, &["rm", "--quiet", "pkg/b.py"]);
    scratch.write("repo/untracked.py", "z = 3\n");

    let walk = scratch.git().tree_walk(&root, &commit, EVERY);

    assert!(walk.warnings.is_empty());
    assert_eq!(listed(walk), owned(&[("a.py", 6), ("pkg/b.py", 7)]));
}

#[test]
fn an_executable_is_listed_and_a_link_and_a_gitlink_are_passed_over() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("run.sh", "echo\n"), ("lib.py", "l = 1\n")]);
    scratch.set_mode("repo/run.sh", 0o755);
    scratch.symlink("repo/link.py", Path::new("lib.py"));
    let commit = head(&scratch, &root);
    let gitlink = format!("160000,{commit},vendor");
    // `add -A` would stage the removal of a gitlink with no directory, so it runs first.
    scratch.run_git(&root, &["add", "-A"]);
    scratch.run_git(&root, &["update-index", "--add", "--cacheinfo", &gitlink]);
    scratch.run_git(&root, &["commit", "--quiet", "-m", "modes"]);

    let commit = head(&scratch, &root);
    let tree = scratch.run_git(&root, &["ls-tree", &commit]);
    let tree = String::from_utf8(tree).expect("a UTF-8 listing");
    assert!(tree.contains("100755 blob") && tree.contains("120000 blob"));
    assert!(tree.contains("160000 commit"));

    let walk = scratch.git().tree_walk(&root, &commit, EVERY);

    assert_eq!(listed(walk), owned(&[("lib.py", 6), ("run.sh", 5)]));
}

#[test]
fn a_root_below_the_top_level_lists_its_own_files_relative_to_it() {
    let scratch = Scratch::new();
    let top = scratch.repo(
        "repo",
        &[("top.py", "t = 1\n"), ("pkg/sub/m.py", "m = 1\n")],
    );
    let commit = head(&scratch, &top);

    let walk = scratch.git().tree_walk(&top.join("pkg"), &commit, EVERY);

    assert_eq!(listed(walk), owned(&[("sub/m.py", 6)]));
}

#[test]
fn the_walk_bounds_refuse_a_tree_with_too_many_files() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "a\n"), ("b.py", "b\n")]);
    let commit = head(&scratch, &root);
    let rest = ["-r", "-z", "--long", commit.as_str()].map(OsStr::new);
    let outcome = scratch.git().run_offline(&root, "ls-tree", &rest, LISTING);
    let bounds = Bounds { files: 1, ..BOUNDS };

    let walk = tree_files(&root, outcome, bounds, EVERY);

    assert!(matches!(
        walk.files,
        Err(WalkError::Refused(io::WalkRefusal::TooManyFiles {
            limit: 1,
            ..
        }))
    ));
}

#[test]
fn the_byte_bound_counts_only_the_files_that_the_scan_reads() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "a = 1\n"), ("big.txt", "0123456789\n")]);
    let commit = head(&scratch, &root);
    let rest = ["-r", "-z", "--long", commit.as_str()].map(OsStr::new);
    let python: io::ScanReads = |path, _| Path::new(path).extension() == Some(OsStr::new("py"));
    let bounds = Bounds { bytes: 6, ..BOUNDS };
    let outcome = scratch.git().run_offline(&root, "ls-tree", &rest, LISTING);

    let walk = tree_files(&root, outcome, bounds, python);

    assert_eq!(listed(walk), owned(&[("a.py", 6), ("big.txt", 11)]));
}

#[test]
fn a_shallow_clone_lists_the_tree_of_the_commit_that_it_holds() {
    let scratch = Scratch::new();
    let source = scratch.repo("source", &[("a.py", "a = 1\n")]);
    scratch.write("source/a.py", "a = 22\n");
    scratch.commit_all(&source, "second");
    let url = format!("file://{}", source.display());
    let clone = scratch.root.join("clone");
    let target = clone.to_str().expect("a UTF-8 scratch path");
    scratch.run_git(
        &scratch.root,
        &["clone", "--quiet", "--depth", "1", &url, target],
    );
    let commit = head(&scratch, &clone);

    let walk = scratch.git().tree_walk(&clone, &commit, EVERY);

    assert_eq!(listed(walk), owned(&[("a.py", 7)]));
}

#[test]
fn a_commit_that_the_repository_lacks_is_unlisted_with_the_git_note() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "a\n")]);
    let missing = "0123456789abcdef0123456789abcdef01234567";

    let walk = scratch.git().tree_walk(&root, missing, EVERY);

    let Err(WalkError::Unlisted { note, .. }) = walk.files else {
        panic!("an unlisted root, got {:?}", walk.files);
    };
    assert!(note.starts_with("git ls-tree exited 128: "), "{note}");
}

#[test]
fn a_blob_read_and_a_tree_read_tell_git_never_to_fetch() {
    let scratch = Scratch::new();
    let body = format!("#!/bin/sh\nprintf '%s' \"${}\"\n", NO_LAZY_FETCH.0);
    let bin = scratch.fake_git(&body, 0o755);
    let git = crate::Git::new(scratch.env_with(&[("PATH", path_before(bin).as_os_str())]));
    let printed = GitOutcome::Printed(NO_LAZY_FETCH.1.as_bytes().to_vec());

    assert_eq!(git.blob(&scratch.root, "HEAD", "a.py"), printed);
    let spec = git.offline_spec(&scratch.root, "ls-tree", &[], LISTING);
    let (name, value) = NO_LAZY_FETCH;
    assert!(
        spec.env
            .iter()
            .any(|(key, set)| key == name && set == value)
    );
}
