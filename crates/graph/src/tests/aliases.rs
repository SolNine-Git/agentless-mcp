//! The alias index lists each member binding whose chain reaches a target once, in path order,
//! and walks every chain once for all the targets that a later fan-in asks about.

use std::collections::BTreeSet;

use super::rust::scanned;
use crate::FileFacts;
use crate::{Aliases, NameIndex, Resolver, Scopes};

const FILES: [(&str, &str); 5] = [
    (
        "pkg/core.py",
        "def target():\n    return 1\n\n\ndef other():\n    return 2\n",
    ),
    ("pkg/mid.py", "from pkg.core import target, other\n"),
    ("pkg/api.py", "from pkg.mid import target\n"),
    ("app/b.py", "from pkg.api import target as run\n"),
    ("app/a.py", "from pkg.mid import other as spare\n"),
];

#[test]
fn each_binding_that_reaches_a_target_is_listed_once_in_path_order() {
    let files = scanned(&FILES);
    let files: Vec<&FileFacts> = files.iter().collect();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let resolver = Resolver::new(&index, &scopes);
    let aliases = Aliases::build(&resolver);
    let walks = resolver.walks();
    let ids = |names: &[&str]| -> BTreeSet<String> {
        let ids = names.iter().map(|name| format!("py:pkg/core.py::{name}"));
        ids.collect()
    };

    let target = aliases.reaching(&ids(&["target"]));
    let both = aliases.reaching(&ids(&["target", "other"]));

    assert_eq!(
        target,
        [
            ("app/b.py", "run"),
            ("pkg/api.py", "target"),
            ("pkg/mid.py", "target")
        ]
    );
    assert_eq!(
        both,
        [
            ("app/a.py", "spare"),
            ("app/b.py", "run"),
            ("pkg/api.py", "target"),
            ("pkg/mid.py", "other"),
            ("pkg/mid.py", "target"),
        ]
    );
    assert!(aliases.reaching(&ids(&["missing"])).is_empty());
    assert_eq!(resolver.walks(), walks, "a lookup walks no chain");
}
