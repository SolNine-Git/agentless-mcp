use std::ffi::OsStr;
use std::path::Path;

use super::{RECEIPT, Snapshot, dirty_count, receipt};
use crate::run::classify;
use crate::testing::{Scratch, cases, latin};
use crate::{Git, GitOutcome, Unfinished};

const SAMPLE: [(&str, &str); 2] = [("a.py", "x = 1\n"), ("sub/b.py", "y = 2\n")];

// One reference case: the input, whether git was cut off, then the text, note, and dirty count.
type ReceiptCase = (String, bool, Option<String>, String, Option<usize>);

fn snapshot_of(git: &Git, root: &Path) -> Snapshot {
    git.snapshot(root, git.root(root).as_deref())
}

#[test]
fn the_receipt_text_and_dirty_count_match_the_reference_cases() {
    let cases: Vec<ReceiptCase> = cases("receipt", "receipt");
    assert!(!cases.is_empty());
    for (raw, truncated, text, note, dirty) in cases {
        let raw = latin(&raw);
        let outcome = if truncated {
            GitOutcome::Capped(raw.clone())
        } else {
            GitOutcome::Printed(raw.clone())
        };
        let read = receipt("status", outcome);
        let expected = text.ok_or(note);
        assert_eq!(read, expected, "input {raw:?}");
        assert_eq!(
            read.ok().map(|text| dirty_count(&text)),
            dirty,
            "input {raw:?}"
        );
    }
}

#[test]
fn a_snapshot_reports_head_tree_and_a_clean_tree() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &SAMPLE);
    let snapshot = snapshot_of(&scratch.git(), &root);

    assert_eq!(snapshot.head_sha.as_deref().map(str::len), Some(8));
    assert_eq!(snapshot.tree_oid.as_deref().map(str::len), Some(8));
    assert_eq!(snapshot.dirty_count, Some(0));
    assert_eq!(snapshot.note, "");
}

#[test]
fn a_snapshot_counts_modified_and_untracked_paths() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &SAMPLE);
    scratch.write("repo/a.py", "x = 2\n");
    scratch.write("repo/new.py", "z = 3\n");

    assert_eq!(snapshot_of(&scratch.git(), &root).dirty_count, Some(2));
}

#[test]
fn a_nested_directory_says_whose_state_it_reported() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &SAMPLE);
    let nested = root.join("sub");
    let snapshot = snapshot_of(&scratch.git(), &nested);

    let expected = format!(
        "{} is not the top of its git repository: HEAD and dirty count describe {}",
        nested.display(),
        root.display()
    );
    assert_eq!(snapshot.note, expected);
}

#[test]
fn a_nested_directory_still_reports_the_head_it_is_cached_under() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &SAMPLE);
    scratch.write("repo/a.py", "x = 2\n");
    let git = scratch.git();
    let nested = snapshot_of(&git, &root.join("sub"));

    assert_eq!(nested.head_sha, snapshot_of(&git, &root).head_sha);
    assert_eq!(nested.dirty_count, Some(1));
}

#[test]
fn a_directory_outside_git_is_all_unknown_with_a_note() {
    let scratch = Scratch::new();
    let plain = scratch.create_dir("plain");
    let snapshot = snapshot_of(&scratch.git(), &plain);

    let note = format!(
        "{} is not inside a git repository: HEAD and dirty count are unknown",
        plain.display()
    );
    let unknown = Snapshot {
        head_sha: None,
        tree_oid: None,
        dirty_count: None,
        note,
    };
    assert_eq!(snapshot, unknown);
}

#[test]
fn a_repository_without_commits_reports_the_reason() {
    let scratch = Scratch::new();
    let root = scratch.init("empty");
    let snapshot = snapshot_of(&scratch.git(), &root);

    assert_eq!(snapshot.head_sha, None);
    assert_eq!(snapshot.dirty_count, Some(0));
    assert!(
        snapshot
            .note
            .starts_with("git rev-parse exited 128: fatal:"),
        "{}",
        snapshot.note
    );
}

#[test]
fn a_missing_git_binary_is_a_note_not_a_failure() {
    let scratch = Scratch::new();
    let empty = scratch.create_dir("empty-bin");
    let git = Git::new(scratch.env_with(&[("PATH", empty.as_os_str())]));

    assert_eq!(git.root(&scratch.root), None);
    let read = git.receipt(&scratch.root, "status", &["--porcelain"]);
    assert_eq!(
        read,
        Err("git is not installed, so repository state is unknown".to_owned())
    );
    assert_eq!(snapshot_of(&git, &scratch.root).dirty_count, None);
}

#[test]
fn a_timeout_leaves_the_dirty_count_unknown() {
    let timed_out = receipt("status", classify("status", RECEIPT, io::Outcome::TimedOut));
    let ok = || Ok("72f5a2ff".to_owned());
    let snapshot = Snapshot::read(Path::new("/repo"), None, ok(), ok(), timed_out);

    assert_eq!(snapshot.dirty_count, None);
    assert_eq!(snapshot.note, "git status timed out after 5.0s");
}

#[test]
fn a_process_that_cannot_be_spawned_degrades_with_its_reason() {
    let spawn = || {
        let error = std::io::Error::from_raw_os_error(24);
        let outcome = io::Outcome::NotRun(io::NotRun::Spawn(error));
        receipt("rev-parse", classify("rev-parse", RECEIPT, outcome))
    };
    let snapshot = Snapshot::read(Path::new("/repo"), None, spawn(), spawn(), spawn());

    assert_eq!(
        (snapshot.head_sha, snapshot.tree_oid, snapshot.dirty_count),
        (None, None, None)
    );
    let note = "git rev-parse could not be run: Too many open files";
    assert_eq!(snapshot.note, [note, note, note].join("; "));
}

#[test]
fn the_note_joins_the_borrowed_note_then_head_tree_and_status() {
    let failed = |note: &str| Err(note.to_owned());
    let snapshot = Snapshot::read(
        Path::new("/r/sub"),
        Some(Path::new("/r")),
        failed("head"),
        failed("tree"),
        failed("status"),
    );

    let borrowed = "/r/sub is not the top of its git repository: HEAD and dirty count describe /r";
    assert_eq!(snapshot.note, format!("{borrowed}; head; tree; status"));
}

#[test]
fn output_past_the_cap_is_unknown_rather_than_partial() {
    let read = receipt("status", GitOutcome::Capped(b" M a.py".to_vec()));
    assert_eq!(
        read,
        Err("git status printed more than 8000000 bytes".to_owned())
    );
    let unfinished = GitOutcome::Unfinished(Unfinished::NotInstalled);
    assert!(receipt("status", unfinished).is_err());
}

#[test]
fn the_root_is_found_from_a_subdirectory_and_from_a_file() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &SAMPLE);
    let git = scratch.git();

    assert_eq!(git.root(&root.join("sub")), Some(root.clone()));
    assert_eq!(git.root(&root.join("a.py")), Some(root));
}

#[test]
fn there_is_no_root_outside_a_repository() {
    let scratch = Scratch::new();
    assert_eq!(scratch.git().root(&scratch.root), None);
}

#[test]
fn a_git_dir_pointing_elsewhere_does_not_move_the_head() {
    let scratch = Scratch::new();
    let analysed = scratch.repo("analysed", &[("m.py", "value = 1\n")]);
    let elsewhere = scratch.repo("elsewhere", &[("n.py", "value = 2\n")]);
    let expected = snapshot_of(&scratch.git(), &analysed).head_sha;
    let other = elsewhere.join(".git");
    let git = Git::new(scratch.env_with(&[("GIT_DIR", other.as_os_str())]));

    assert_eq!(snapshot_of(&git, &analysed).head_sha, expected);
}

#[test]
fn a_broken_index_file_does_not_make_a_clean_tree_dirty() {
    let scratch = Scratch::new();
    let analysed = scratch.repo("analysed", &[("m.py", "value = 1\n")]);
    let missing = scratch.root.join("nonexistent");
    let git = Git::new(scratch.env_with(&[("GIT_INDEX_FILE", missing.as_os_str())]));

    assert_eq!(snapshot_of(&git, &analysed).dirty_count, Some(0));
}

#[test]
fn the_kept_names_cannot_redirect_git_at_another_repository() {
    let scratch = Scratch::new();
    let analysed = scratch.repo("analysed", &[("m.py", "value = 1\n")]);
    let elsewhere = scratch.repo("elsewhere", &[("n.py", "value = 2\n")]);
    let expected = snapshot_of(&scratch.git(), &analysed).head_sha;
    let config = elsewhere.join(".git").join("config");
    let git = Git::new(scratch.env_with(&[
        ("GIT_CONFIG_GLOBAL", config.as_os_str()),
        ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
    ]));

    assert_eq!(snapshot_of(&git, &analysed).head_sha, expected);
}

// Rewrites HEAD with a gpgsig header, so that git offers the commit to the signing program.
fn sign_head_commit(scratch: &Scratch, root: &Path) {
    let commit = String::from_utf8(scratch.run_git(root, &["cat-file", "commit", "HEAD"]))
        .expect("a UTF-8 commit object");
    let mut lines = Vec::new();
    for line in commit.split('\n') {
        lines.push(line.to_owned());
        if line.starts_with("committer ") {
            let signature = [
                "gpgsig -----BEGIN PGP SIGNATURE-----",
                " ",
                " bm90IGEgc2lnbmF0dXJl",
                " -----END PGP SIGNATURE-----",
            ];
            lines.extend(signature.map(str::to_owned));
        }
    }
    let object = scratch.write("signed-commit", &lines.join("\n"));
    let object = object.to_str().expect("a UTF-8 scratch path");
    let written = scratch.run_git(root, &["hash-object", "-t", "commit", "-w", object]);
    let written = String::from_utf8(written).expect("an object name");
    scratch.run_git(root, &["update-ref", "HEAD", written.trim()]);
}

#[test]
fn the_snapshot_reads_run_no_program_a_repository_names() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &SAMPLE);
    let marker = scratch.root.join("gpg-fired.txt");
    let body = format!(
        "#!/bin/sh\necho fired >> \"{}\"\ncat > /dev/null\n",
        marker.display()
    );
    let program = scratch.write("gpg.sh", &body);
    scratch.set_mode("gpg.sh", 0o755);
    sign_head_commit(&scratch, &root);
    let program = program.to_str().expect("a UTF-8 scratch path");
    scratch.run_git(&root, &["config", "log.showSignature", "true"]);
    scratch.run_git(&root, &["config", "gpg.program", program]);
    // The control: without the hardening prefix, the repository's program runs.
    scratch.run_git(&root, &["log", "--oneline"]);
    let fired = || {
        std::fs::read_to_string(&marker)
            .expect("the bait fired once")
            .lines()
            .count()
    };
    let before = fired();

    assert!(snapshot_of(&scratch.git(), &root).head_sha.is_some());
    assert_eq!(
        fired(),
        before,
        "the repository's gpg.program ran under the snapshot"
    );
}
