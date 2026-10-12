use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{BOUNDS, Bounds, WalkError, WalkWarning, admit, ignore_verdict, listed_names};
use crate::testing::{Scratch, cases, latin, path_before};
use crate::{Failure, Git, GitOutcome, Unfinished};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
enum Decoded {
    Listed {
        paths: Vec<String>,
        undecodable: Vec<usize>,
    },
    Refused {
        refused: String,
    },
}

// A repository with one ignored file, one ignored directory, and one nested package.
fn git_repo(scratch: &Scratch) -> PathBuf {
    scratch.repo(
        "repo",
        &[
            (".gitignore", "build/\n*.log\n"),
            ("app.py", "x = 1\n"),
            ("pkg/mod.py", "y = 2\n"),
            ("debug.log", "noise\n"),
            ("build/artifact.bin", "binary\n"),
        ],
    )
}

// A repository that ignores one inner tree of files and owns another.
fn disowning_repo(scratch: &Scratch) -> PathBuf {
    scratch.repo(
        "outer",
        &[
            (".gitignore", "repos/\n*.log\n"),
            ("outer.py", "o = 1\n"),
            ("kept/lib.py", "l = 1\n"),
            ("kept/trace.log", "noise\n"),
            ("repos/snap/app.py", "a = 1\n"),
            ("repos/snap/pkg/mod.py", "m = 2\n"),
        ],
    )
}

fn paths(scratch: &Scratch, root: &Path) -> Vec<String> {
    let walk = scratch
        .git()
        .walk(root)
        .unwrap_or_else(|error| panic!("{error}"));
    walk.files.into_iter().map(|file| file.path).collect()
}

fn shown(warnings: &[WalkWarning]) -> Vec<String> {
    warnings.iter().map(ToString::to_string).collect()
}

#[test]
fn the_decoded_listing_matches_the_generated_cases() {
    let cases: Vec<(String, usize, Decoded)> = cases("walk", "decoded_paths");
    assert!(!cases.is_empty());
    for (raw, max_files, expected) in cases {
        let raw = latin(&raw);
        let outcome = GitOutcome::Printed(raw.clone());
        let decoded = match listed_names(Path::new("/repo"), outcome, max_files) {
            Ok((paths, warnings)) => {
                let count = |warning: &WalkWarning| match warning {
                    WalkWarning::NotUtf8 { count, .. } => *count,
                    other => panic!("an unexpected warning: {other}"),
                };
                let undecodable = warnings.iter().map(count).collect();
                Decoded::Listed { paths, undecodable }
            }
            Err(error) => Decoded::Refused {
                refused: error.to_string(),
            },
        };
        assert_eq!(decoded, expected, "input {raw:?}");
    }
}

#[test]
fn gitignored_paths_are_excluded() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    assert_eq!(
        paths(&scratch, &root),
        [".gitignore", "app.py", "pkg/mod.py"]
    );
}

#[test]
fn an_untracked_file_that_no_rule_ignores_is_included() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    scratch.write("repo/new.py", "z = 3\n");
    let listed = paths(&scratch, &root);
    assert!(listed.contains(&"new.py".to_owned()), "{listed:?}");
    assert!(!listed.contains(&"debug.log".to_owned()), "{listed:?}");
}

#[test]
fn a_directory_outside_git_falls_back_to_the_bounded_walk() {
    let scratch = Scratch::new();
    scratch.write("plain/a.py", "a = 1\n");
    scratch.write("plain/sub/b.py", "b = 2\n");
    // Not a git repository, so .gitignore is just another file.
    scratch.write("plain/.gitignore", "*.py\n");
    let root = scratch.root.join("plain");
    assert_eq!(paths(&scratch, &root), [".gitignore", "a.py", "sub/b.py"]);
}

#[test]
fn sizes_are_reported() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    let walk = scratch.git().walk(&root).expect("a listed repository");
    let app = walk.files.iter().find(|file| file.path == "app.py");
    assert_eq!(app.map(|file| file.size), Some(6));
}

#[test]
fn the_file_bound_is_enforced_inside_a_repository() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    let bounds = Bounds { files: 2, ..BOUNDS };
    let walked = scratch.git().walk_within(&root, bounds);
    let error = walked.expect_err("three files pass a bound of two");
    let expected = format!(
        "walk refused: more than 2 files under {}; point the call at a subdirectory or raise the \
         file bound",
        root.display()
    );
    assert_eq!(error.to_string(), expected);
}

#[test]
fn a_tracked_symlink_escaping_the_root_is_not_listed() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    let secret = scratch.write("secret.txt", "classified\n");
    scratch.symlink("repo/leak.py", &secret);
    scratch.commit_all(&root, "leak");

    let listed = paths(&scratch, &root);
    assert!(!listed.contains(&"leak.py".to_owned()), "{listed:?}");
}

#[test]
fn a_subdirectory_root_still_honours_the_repository_gitignore() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    scratch.write("repo/pkg/trace.log", "noise\n");
    assert_eq!(paths(&scratch, &root.join("pkg")), ["mod.py"]);
}

#[test]
fn a_file_that_is_gone_before_the_stat_is_skipped_and_named() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    // The containment check passes, as it does when the file goes between the two calls.
    let names = vec!["gone.py".to_owned(), "pkg/mod.py".to_owned()];

    let (files, warnings) =
        admit(&root, names, BOUNDS, |_| true, io::file_size).expect("within the bounds");
    let listed: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(listed, ["pkg/mod.py"]);
    let gone = root.join("gone.py");
    let expected = format!("walk skipped {}: No such file or directory", gone.display());
    assert_eq!(shown(&warnings), [expected]);
}

#[test]
fn a_path_deeper_than_the_bound_is_refused() {
    let names = vec!["a/b/c.py".to_owned()];
    let bounds = Bounds { depth: 1, ..BOUNDS };
    let error = admit(Path::new("/repo"), names, bounds, |_| true, |_| Ok(1))
        .expect_err("depth 2 passes a bound of 1");
    let expected = "walk refused: path depth 2 exceeds the limit of 1 under /repo; point the \
                    call at a subdirectory instead";
    assert_eq!(error.to_string(), expected);
    assert!(matches!(
        kernel::Error::from(error),
        kernel::Error::WalkBoundExceeded(_)
    ));
}

#[test]
fn the_byte_bound_is_enforced() {
    let names = vec!["a.py".to_owned(), "b.py".to_owned()];
    let bounds = Bounds { bytes: 5, ..BOUNDS };
    let error = admit(Path::new("/repo"), names, bounds, |_| true, |_| Ok(3))
        .expect_err("6 bytes pass a bound of 5");
    let expected = "walk refused: more than 5 bytes under /repo; point the call at a \
                    subdirectory or raise the byte bound";
    assert_eq!(error.to_string(), expected);
}

#[test]
fn a_root_the_enclosing_repository_ignores_is_walked() {
    let scratch = Scratch::new();
    let outer = disowning_repo(&scratch);
    assert_eq!(
        paths(&scratch, &outer.join("repos/snap")),
        ["app.py", "pkg/mod.py"]
    );
}

#[test]
fn a_root_the_enclosing_repository_owns_still_uses_the_listing() {
    let scratch = Scratch::new();
    let outer = disowning_repo(&scratch);
    assert_eq!(paths(&scratch, &outer.join("kept")), ["lib.py"]);
}

#[test]
fn an_ignored_root_with_its_own_repository_uses_its_own_listing() {
    let scratch = Scratch::new();
    let outer = disowning_repo(&scratch);
    let files = [
        (".gitignore", "*.tmp\n"),
        ("keep.py", "k = 1\n"),
        ("scratch.tmp", "noise\n"),
    ];
    let inner = scratch.repo("outer/repos/own", &files);
    assert!(inner.starts_with(&outer));
    assert_eq!(paths(&scratch, &inner), [".gitignore", "keep.py"]);
}

#[test]
fn a_git_that_cannot_be_run_is_a_typed_refusal() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nexit 0\n", 0o644);
    let git = Git::new(scratch.env_with(&[("PATH", bin.as_os_str())]));
    let error = git.listed(&scratch.root, 10).expect_err("git cannot run");

    let expected = format!(
        "{} cannot be listed the way its .gitignore asks: git ls-files could not be run: \
         Permission denied",
        scratch.root.display()
    );
    assert_eq!(error.to_string(), expected);
    assert!(matches!(
        kernel::Error::from(error),
        kernel::Error::RepoResolution(_)
    ));
}

#[test]
fn a_listing_over_the_output_cap_is_refused_rather_than_cut() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nexec head -c 16000001 /dev/zero\n", 0o755);
    let path = path_before(bin);
    let git = Git::new(scratch.env_with(&[("PATH", path.as_os_str())]));
    let error = git
        .listed(&scratch.root, 10)
        .expect_err("the listing passes its cap");

    let expected = format!(
        "walk refused: git listed more than 16000000 bytes of names under {}; point the call at \
         a subdirectory instead",
        scratch.root.display()
    );
    assert_eq!(error.to_string(), expected);
    assert!(matches!(
        kernel::Error::from(error),
        kernel::Error::WalkBoundExceeded(_)
    ));
}

#[test]
fn a_name_that_is_not_utf8_reaches_the_decoder_as_raw_bytes() {
    let scratch = Scratch::new();
    let root = git_repo(&scratch);
    scratch.write_bytes(OsStr::from_bytes(b"repo/bad\xff.py"), b"x = 1\n");
    let walk = scratch.git().walk(&root).expect("a listed repository");

    let listed: Vec<&str> = walk.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(listed, [".gitignore", "app.py", "pkg/mod.py"]);
    let expected = format!(
        "git listed 1 path(s) under {} whose names are not valid UTF-8. The tree omits them.",
        root.display()
    );
    assert_eq!(shown(&walk.warnings), [expected]);
}

#[test]
fn the_bounded_walk_drops_a_name_that_is_not_utf8_with_a_warning() {
    let scratch = Scratch::new();
    scratch.write("plain/a.py", "a = 1\n");
    scratch.write_bytes(OsStr::from_bytes(b"plain/bad\xff.py"), b"x = 1\n");
    let root = scratch.root.join("plain");
    let walk = scratch.git().walk(&root).expect("a walked tree");

    let listed: Vec<&str> = walk.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(listed, ["a.py"]);
    let expected = format!(
        "the walk found 1 path(s) under {} whose names are not valid UTF-8. The tree omits them.",
        root.display()
    );
    assert_eq!(shown(&walk.warnings), [expected]);
}

#[test]
fn both_walk_calls_carry_the_prefix_and_the_scrubbed_environment() {
    let scratch = Scratch::new();
    let body = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\nenv > \"$ENV_LOG\"\nexit 1\n";
    let path = path_before(scratch.fake_git(body, 0o755));
    let (argv_log, env_log) = (scratch.root.join("argv.log"), scratch.root.join("env.log"));
    let git = Git::new(scratch.env_with(&[
        ("PATH", path.as_os_str()),
        ("ARGV_LOG", argv_log.as_os_str()),
        ("ENV_LOG", env_log.as_os_str()),
        ("GIT_DIR", OsStr::new("/somewhere/else")),
    ]));
    let (top, root) = (scratch.root.as_path(), scratch.root.join("sub"));
    let root_text = root.to_str().expect("a UTF-8 scratch path");

    assert!(!git.ignores(top, &root).0);
    let argv = fs::read_to_string(&argv_log).expect("the fake git wrote its argv");
    assert!(argv.starts_with("--no-optional-locks\n"), "{argv}");
    assert!(
        argv.ends_with(&format!("check-ignore\n-q\n--\n{root_text}\n")),
        "{argv}"
    );
    assert_hardened(&env_log);

    assert!(git.listed(&root, 10).is_err());
    let argv = fs::read_to_string(&argv_log).expect("the fake git wrote its argv");
    let listing = "ls-files\n--cached\n--others\n--exclude-standard\n-z\n";
    assert!(
        argv.starts_with("--no-optional-locks\n") && argv.ends_with(listing),
        "{argv}"
    );
    assert_hardened(&env_log);
}

fn assert_hardened(env_log: &Path) {
    let env = fs::read_to_string(env_log).expect("the fake git wrote its environment");
    let lines: Vec<&str> = env.lines().collect();
    assert!(lines.contains(&"LC_ALL=C"), "{lines:?}");
    let kept = ["GIT_CONFIG_GLOBAL=", "GIT_CONFIG_NOSYSTEM="];
    let stray =
        |line: &&&str| line.starts_with("GIT_") && !kept.iter().any(|k| line.starts_with(k));
    let strays: Vec<&&str> = lines.iter().filter(stray).collect();
    assert!(strays.is_empty(), "{strays:?}");
}

#[test]
fn an_unanswered_ignore_check_keeps_the_listing_and_warns() {
    let reason = Unfinished::TimedOut {
        subcommand: "check-ignore",
        after: "30",
    };
    let (top, root) = (Path::new("/top"), Path::new("/top/root"));
    let (ignored, warning) = ignore_verdict(top, root, GitOutcome::Unfinished(reason));

    assert!(!ignored);
    let expected = "git check-ignore could not be run in /top for /top/root (git check-ignore \
                    timed out after 30s). The listing for /top/root comes from git, as it did \
                    before.";
    assert_eq!(
        warning.map(|warning| warning.to_string()).as_deref(),
        Some(expected)
    );
}

#[test]
fn only_exit_zero_reads_as_ignored() {
    let (top, root) = (Path::new("/top"), Path::new("/top/root"));
    let failure = Failure {
        subcommand: "check-ignore",
        code: 1,
        detail: "no detail".to_owned(),
    };
    let verdict = |outcome| {
        let (ignored, warning) = ignore_verdict(top, root, outcome);
        (ignored, warning.is_some())
    };
    assert_eq!(verdict(GitOutcome::Printed(Vec::new())), (true, false));
    assert_eq!(verdict(GitOutcome::Failed(failure)), (false, false));
    assert_eq!(verdict(GitOutcome::Capped(Vec::new())), (false, false));
}

#[test]
fn a_root_that_is_not_a_directory_is_refused() {
    let scratch = Scratch::new();
    let file = scratch.write("file.py", "x = 1\n");
    let error = scratch.git().walk(&file).expect_err("a file is no root");

    assert_eq!(
        error.to_string(),
        format!("not a directory: {}", file.display())
    );
    assert!(matches!(
        error,
        WalkError::Refused(io::WalkRefusal::NotDirectory { .. })
    ));
}
