use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use git::Toplevel;
use io::testing::cases;

use super::{anchor, found, halt_for, old_text, relative, require_tier1};
use crate::record::Outcome;
use crate::testing::{MANIFEST_DIR, Sandbox};

#[test]
fn the_tier1_rule_matches_the_reference_cases() {
    let all: Vec<(String, bool)> = cases(MANIFEST_DIR, "target", "tier1");
    assert!(!all.is_empty());
    for (name, expected) in all {
        let found = require_tier1(&Path::new("/repo").join(&name));
        assert_eq!(found.is_ok(), expected, "name {name:?}: {found:?}");
        if let Err(halt) = found {
            assert_eq!(halt.outcome, Outcome::Skipped);
            assert_eq!(halt.reason, format!("not a tier-1 source file: {name}"));
        }
    }
}

#[test]
fn every_suffix_the_reference_hook_gates_is_tier1_here() {
    let all: Vec<(String,)> = cases(MANIFEST_DIR, "target", "suffixes");
    assert_eq!(all.len(), 20);
    for (suffix,) in all {
        assert!(
            require_tier1(Path::new(&format!("/r/x{suffix}"))).is_ok(),
            "{suffix}"
        );
    }
}

#[test]
fn the_path_below_the_root_matches_the_reference_cases() {
    let all: Vec<(String, String, Option<String>)> = cases(MANIFEST_DIR, "target", "relative");
    assert!(!all.is_empty());
    for (resolved, root, expected) in all {
        let found = relative(Path::new(&resolved), Path::new(&root));
        assert_eq!(
            found.as_ref().ok(),
            expected.as_ref(),
            "{resolved} below {root}"
        );
        if let Err(halt) = found {
            assert_eq!(halt.reason, "the resolved target is outside the repository");
        }
    }
}

#[test]
fn each_answer_of_rev_parse_gives_its_halt() {
    let budget = Duration::from_secs(2);
    let failed = Toplevel::Failed {
        code: 128,
        stderr: b"warning: x\nfatal: no access\n".to_vec(),
    };
    let not_run = |errno| Toplevel::NotRun {
        errno,
        message: "Permission denied".to_owned(),
    };
    let degraded = [
        (failed, "git rev-parse exited 128: fatal: no access"),
        (
            Toplevel::Capped,
            "git rev-parse printed more than 65536 bytes",
        ),
        (Toplevel::TimedOut, "git rev-parse timed out after 2.0 s"),
        (
            Toplevel::NotInstalled,
            "cannot run git: [Errno 2] No such file or directory: 'git'",
        ),
        (
            not_run(Some(13)),
            "cannot run git: [Errno 13] Permission denied: 'git'",
        ),
        (not_run(None), "cannot run git: Permission denied"),
    ];
    for (toplevel, reason) in degraded {
        let halt = halt_for(toplevel, budget).expect_err("no top level");
        assert_eq!(
            (halt.outcome, halt.reason.as_str()),
            (Outcome::Degraded, reason)
        );
    }
    let halt = halt_for(Toplevel::NotRepository, budget).expect_err("no repository");
    assert_eq!(
        (halt.outcome, halt.reason.as_str()),
        (Outcome::Skipped, "not in a git repository")
    );
    let printed = halt_for(Toplevel::Found(b"/r\n".to_vec()), budget);
    assert_eq!(printed, Ok(b"/r\n".to_vec()));
}

#[test]
fn a_top_level_that_is_not_utf8_is_an_internal_error() {
    let halt = found(b"/tmp/r\xff/repo\n").expect_err("no UTF-8 path");
    let detail = "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 6: \
                  invalid start byte";
    assert_eq!(halt.internal.as_deref(), Some(detail));
}

#[test]
fn a_tier1_file_below_a_directory_that_is_not_utf8_is_degraded() {
    let resolved = Path::new(OsStr::from_bytes(b"/repo/d\xff/a.py"));
    let halt = relative(resolved, Path::new("/repo")).expect_err("no UTF-8 path");
    assert_eq!(halt.outcome, Outcome::Degraded);
    assert_eq!(halt.reason, "lint failed: not a UTF-8 path: d\u{fffd}/a.py");
}

#[test]
fn a_target_under_an_unsearchable_directory_is_degraded_with_cannot_stat() {
    let sandbox = Sandbox::new();
    if !sandbox.modes_bind() {
        return;
    }
    let target = sandbox.put("locked/a.py", "x = 1\n");
    sandbox.set_mode("locked", 0o000);
    let found = old_text(&target);
    sandbox.set_mode("locked", 0o755);

    let halt = found.expect_err("no stat");
    let reason = format!(
        "cannot stat the target: [Errno 13] Permission denied: '{}'",
        target.display()
    );
    assert_eq!((halt.outcome, halt.reason), (Outcome::Degraded, reason));
}

#[test]
fn the_anchor_is_the_nearest_existing_directory() {
    let sandbox = Sandbox::new();
    let made = sandbox.mkdir("a/b");
    let given = made.join("missing/deeper/x.py");
    assert_eq!(anchor(&given), made);
    assert_eq!(anchor(Path::new("x.py")), PathBuf::from("."));
}
