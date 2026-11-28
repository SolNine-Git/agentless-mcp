use std::fs;
use std::path::Path;

use super::{TRACKED, TrackedError};
use crate::run::classify;
use crate::testing::Scratch;
use crate::{Git, GitOutcome, Unfinished};

#[test]
fn a_real_index_holds_a_committed_file_its_folders_and_each_letter_case_of_them() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[(".venv/bin/ruff", "#!/bin/sh\n")]);
    scratch.write("repo/other/ruff", "#!/bin/sh\n");
    let git = scratch.git();
    let held = [".venv/bin/ruff", ".venv", ".VENV/Bin/RUFF"];
    let not_held = ["other/ruff", "other", ".venv/bin/ruf", "missing"];
    for (paths, expected) in [(&held[..], true), (&not_held[..], false)] {
        for path in paths {
            let answer = git.tracks(&root, Path::new(path));
            assert_eq!(answer, Ok(expected), "{path}");
        }
    }
}

#[test]
fn the_call_sends_one_literal_pathspec_that_ignores_letter_case_after_the_directory() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\n", 0o755);
    let log = scratch.root.join("argv.log");
    let env = [("PATH", bin.as_os_str()), ("ARGV_LOG", log.as_os_str())];
    let git = Git::new(scratch.env_with(&env));

    assert_eq!(git.tracks(&scratch.root, Path::new("a/*.py")), Ok(false));

    let argv = fs::read_to_string(&log).expect("the fake git wrote its argv");
    let directory = scratch.root.to_str().expect("a UTF-8 scratch path");
    let expected = [
        "-C",
        directory,
        "ls-files",
        "--cached",
        "-z",
        "--",
        ":(literal,icase)a/*.py",
    ];
    let lines: Vec<&str> = argv.lines().collect();
    assert!(lines.ends_with(&expected), "{lines:?}");
}

#[test]
fn a_listing_answers_and_a_git_with_no_listing_is_an_error() {
    let printing = [
        ("exit 0", Some(false)),
        ("printf 'a\\0'", Some(true)),
        ("head -c 65537 /dev/zero", Some(true)),
        ("exit 1", None),
    ];
    for (body, expected) in printing {
        let scratch = Scratch::new();
        let git = scratch.faked_git(&format!("#!/bin/sh\n{body}\n"), 0o755);
        let answer = git.tracks(&scratch.root, Path::new("a.py"));
        match expected {
            Some(tracked) => assert_eq!(answer, Ok(tracked), "{body}"),
            None => assert!(matches!(answer, Err(TrackedError::Failed(_))), "{body}"),
        }
    }
    let scratch = Scratch::new();
    let outside = scratch.git().tracks(&scratch.root, Path::new("a.py"));
    let Err(TrackedError::Failed(failure)) = outside else {
        panic!("outside a repository git fails, not {outside:?}");
    };
    assert_eq!(failure.code, 128);
    let empty = scratch.create_dir("empty-bin");
    let absent = Git::new(scratch.env_with(&[("PATH", empty.as_os_str())]));
    let missing = absent.tracks(&scratch.root, Path::new("a.py"));
    assert_eq!(
        missing,
        Err(TrackedError::Unfinished(Unfinished::NotInstalled))
    );
}

#[test]
fn a_timed_out_call_names_its_deadline() {
    let GitOutcome::Unfinished(unfinished) = classify("ls-files", TRACKED, io::Outcome::TimedOut)
    else {
        panic!("a deadline is unfinished");
    };
    let error = TrackedError::Unfinished(unfinished);
    assert_eq!(error.to_string(), "git ls-files timed out after 5.0s");
}
