use git::RepoFile;

use super::{TreeRequest, render_tree, tree};
use crate::testing::Sandbox;
use crate::{Error, IntArg, Repo};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_tree_text_is_the_reference_text() {
    let cases: Vec<(Vec<String>, usize, usize, String)> =
        io::testing::cases(MANIFEST_DIR, "read/views", "tree");
    assert!(!cases.is_empty());
    for (paths, depth, max_entries, expected) in cases {
        let files: Vec<RepoFile> = paths
            .iter()
            .map(|path| RepoFile {
                path: path.clone(),
                size: 0,
            })
            .collect();
        let rendered = render_tree(&files, depth, max_entries);
        assert_eq!(
            rendered, expected,
            "{paths:?} depth {depth} entries {max_entries}"
        );
    }
}

fn request(depth: &str, max_entries: &str) -> TreeRequest {
    let arg = |text: &str| IntArg::parse(text).unwrap_or_else(|| panic!("{text:?}"));
    TreeRequest {
        depth: arg(depth),
        max_entries: arg(max_entries),
    }
}

#[test]
fn the_tree_of_a_repository_lists_its_walked_files() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", "x = 1\n"), ("pkg/b.py", "y = 2\n")]);
    let services = sandbox.services(&root);
    let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
    let noted = tree(&services, &repo, &request("4", "500"));
    assert!(noted.diagnostics.is_empty(), "{:?}", noted.diagnostics);
    let view = noted.value.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(view.text, "a.py\npkg/\n    b.py\n");
    assert_eq!((view.file_count, view.depth, view.max_entries), (2, 4, 500));
}

#[test]
fn a_bound_refusal_comes_before_the_walk() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", "x = 1\n")]);
    let services = sandbox.services(&root);
    let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
    let noted = tree(&services, &repo, &request("0", "500"));
    let Err(Error::Kind(kernel::Error::OperationFailed(message))) = noted.value else {
        panic!("a bound refusal");
    };
    assert_eq!(message, "depth takes a value from 1 through 20, got 0");
    let noted = tree(&services, &repo, &request("4", "20001"));
    assert!(noted.value.is_err());
}
