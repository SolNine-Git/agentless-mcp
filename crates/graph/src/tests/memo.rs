//! The resolver walks an alias chain once for each spelling file and name, however many sites
//! spell the name there.

use super::rust::scanned;
use crate::FileFacts;
use crate::{NameIndex, Resolver, Scopes, SymbolGraph, Tier};

const USERS: usize = 5;
const SITES: usize = 7;

// `target` defined in pkg/core.py and re-exported twice, and USERS files that import it from the
// end of the chain and call it SITES times each.
fn chained() -> Vec<(String, String)> {
    let mut files = vec![
        (
            "pkg/core.py".to_owned(),
            "def target():\n    return 1\n".to_owned(),
        ),
        (
            "pkg/mid.py".to_owned(),
            "from pkg.core import target\n".to_owned(),
        ),
        (
            "pkg/api.py".to_owned(),
            "from pkg.mid import target\n".to_owned(),
        ),
    ];
    for user in 0..USERS {
        let calls = "def run():\n".to_owned() + &"    target()\n".repeat(SITES);
        let text = format!("from pkg.api import target\n\n{calls}");
        files.push((format!("app/user{user}.py"), text));
    }
    files
}

#[test]
fn each_spelling_file_walks_the_chain_once_for_all_its_sites() {
    let written = chained();
    let pairs: Vec<(&str, &str)> = (written.iter())
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let files = scanned(&pairs);
    let files: Vec<&FileFacts> = files.iter().collect();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let resolver = Resolver::new(&index, &scopes);
    let graph = SymbolGraph::build(&resolver, None).expect("no deadline");
    let bound: Vec<_> = (graph.edges().iter())
        .filter(|edge| edge.name() == "target" && edge.source().path().starts_with("app/"))
        .collect();
    assert_eq!(
        bound.len(),
        USERS,
        "one deduplicated edge from each user's run"
    );
    assert!(bound.iter().all(|edge| edge.tier() == Tier::Imported));
    assert!(
        bound
            .iter()
            .all(|edge| edge.target().path() == "pkg/core.py")
    );
    assert_eq!(resolver.walks(), USERS);
}
