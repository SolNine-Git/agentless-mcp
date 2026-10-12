use super::cases::{Scratch, cases};
use crate::{Discarded, discard};

// The setups whose failure comes from a directory mode: the lock open, then the unlink.
const MODE_SETUPS: [&str; 2] = ["read_only", "unlink_refused"];

#[test]
fn each_discard_gives_the_reference_warning_and_leaves_the_reference_files() {
    for (setup, warnings, remaining) in cases().discards {
        let scratch = Scratch::new();
        // A process that file modes do not refuse removes the database in a mode setup.
        let refusable = !MODE_SETUPS.contains(&setup.as_str()) || scratch.tree.modes_bind();
        let discarded = discarded(&scratch, &setup);
        if refusable {
            check_reference(&scratch, &setup, &discarded, (&warnings, &remaining));
        } else {
            assert!(
                matches!(discarded, Discarded::Removed),
                "{setup}: {discarded:?}"
            );
        }
    }
}

fn check_reference(
    scratch: &Scratch,
    setup: &str,
    discarded: &Discarded,
    (warnings, remaining): (&[String], &[String]),
) {
    let kind = match discarded {
        Discarded::Removed => "removed",
        Discarded::LeftInPlace => "left in place",
        Discarded::Failed(_) => "failed",
    };
    let expected_kind = match setup {
        "locked" => "left in place",
        _ if MODE_SETUPS.contains(&setup) => "failed",
        _ => "removed",
    };
    assert_eq!(kind, expected_kind, "{setup}");
    let database = scratch.database.display().to_string();
    let shown: Vec<String> = match discarded {
        Discarded::Failed(failure) => vec![failure.to_string().replace(&database, "{db}")],
        _ => Vec::new(),
    };
    assert_eq!(shown, warnings, "{setup}");
    assert_eq!(names(scratch), remaining, "{setup}");
}

fn discarded(scratch: &Scratch, setup: &str) -> Discarded {
    let tree = &scratch.tree;
    if setup == "sidecars" {
        tree.write_file("tags.db-wal", "").expect("a sidecar");
        tree.write_file("tags.db-shm", "").expect("a sidecar");
    }
    if setup == "unlink_refused" {
        tree.write_file("write.lock", "").expect("a lock file");
    }
    let lock = (setup == "locked")
        .then(|| io::try_lock(&tree.join("write.lock")).expect("the test takes the lock"));
    if MODE_SETUPS.contains(&setup) {
        tree.set_mode(".", 0o555).expect("the mode is set");
    }
    let discarded = discard(&scratch.database);
    tree.set_mode(".", 0o700).expect("the mode is set back");
    drop(lock);
    discarded
}

fn names(scratch: &Scratch) -> Vec<String> {
    let entries = std::fs::read_dir(scratch.tree.path()).expect("the directory is read");
    let mut names: Vec<String> = entries
        .map(|entry| {
            let entry = entry.expect("an entry");
            entry.file_name().to_string_lossy().into_owned()
        })
        .collect();
    names.sort();
    names
}
