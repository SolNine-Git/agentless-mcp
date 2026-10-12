use std::path::{Path, PathBuf};

use io::testing::cases;

use super::{anchor, relative, require_tier1};
use crate::record::Outcome;
use crate::testing::{MANIFEST_DIR, Sandbox};

#[test]
fn the_tier1_rule_matches_the_generated_cases() {
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
fn every_suffix_the_python_hook_gates_is_tier1_here() {
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
fn the_path_below_the_root_matches_the_generated_cases() {
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
fn the_anchor_is_the_nearest_existing_directory() {
    let sandbox = Sandbox::new();
    let made = sandbox.mkdir("a/b");
    let given = made.join("missing/deeper/x.py");
    assert_eq!(anchor(&given), made);
    assert_eq!(anchor(Path::new("x.py")), PathBuf::from("."));
}
