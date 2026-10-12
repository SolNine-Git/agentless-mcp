use super::cases::{Scratch, cases};
use crate::{Discarded, discard};

#[test]
fn each_discard_gives_the_reference_warning_and_leaves_the_reference_files() {
    for (setup, warnings, remaining) in cases().discards {
        let scratch = Scratch::new();
        let discarded = discarded(&scratch, &setup);
        let kind = match &discarded {
            Discarded::Removed => "removed",
            Discarded::LeftInPlace => "left in place",
            Discarded::Failed(_) => "failed",
        };
        let expected_kind = match setup.as_str() {
            "locked" => "left in place",
            "read_only" => "failed",
            _ => "removed",
        };
        assert_eq!(kind, expected_kind, "{setup}");
        let database = scratch.database.display().to_string();
        let shown: Vec<String> = match &discarded {
            Discarded::Failed(failure) => vec![failure.to_string().replace(&database, "{db}")],
            _ => Vec::new(),
        };
        assert_eq!(shown, warnings, "{setup}");
        assert_eq!(names(&scratch), remaining, "{setup}");
    }
}

fn discarded(scratch: &Scratch, setup: &str) -> Discarded {
    let tree = &scratch.tree;
    if setup == "sidecars" {
        tree.write_file("tags.db-wal", "").expect("a sidecar");
        tree.write_file("tags.db-shm", "").expect("a sidecar");
    }
    let lock = (setup == "locked")
        .then(|| io::try_lock(&tree.join("write.lock")).expect("the test takes the lock"));
    if setup == "read_only" {
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
