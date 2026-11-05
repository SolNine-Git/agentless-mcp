//! `from P import N` with a submodule N: the edge to P/__init__.py holds only for an importer
//! outside package P, or for a member that is not a module.

use super::rust::scanned;
use crate::{Scopes, SymbolGraph, import_chain};

const PACKAGE: [(&str, &str); 6] = [
    ("pkg/__init__.py", "name = 1\n"),
    ("pkg/mod.py", "y = 2\n"),
    ("pkg/sub/__init__.py", ""),
    ("pkg/sub/deep.py", "z = 3\n"),
    ("pkgx/__init__.py", ""),
    ("main.py", "x = 0\n"),
];

// The `(module, target)` rows of each statement of the file `path` written as `text`.
fn rows(path: &str, text: &str) -> Vec<(usize, String, String)> {
    let mut files = PACKAGE.to_vec();
    files.retain(|(held, _)| *held != path);
    files.push((path, text));
    let files = scanned(&files);
    let scopes = Scopes::build(&files);
    let scope = scopes.get(path).expect("a scanned file");
    let found = scope.resolved_statements(path);
    found
        .map(|(statement, module, target)| (statement, module.to_owned(), target.to_owned()))
        .collect()
}

fn row(statement: usize, module: &str, target: &str) -> (usize, String, String) {
    (statement, module.to_owned(), target.to_owned())
}

#[test]
fn a_module_inside_the_package_imports_only_the_submodules_it_names() {
    assert_eq!(
        rows("pkg/a.py", "from pkg import mod\nfrom . import sub\n"),
        [
            row(0, "pkg.mod", "pkg/mod.py"),
            row(1, "sub", "pkg/sub/__init__.py")
        ]
    );
    assert_eq!(
        rows("pkg/sub/b.py", "from pkg import mod\nfrom .. import mod\n"),
        [row(0, "pkg.mod", "pkg/mod.py"), row(1, "mod", "pkg/mod.py")]
    );
}

#[test]
fn an_importer_outside_the_package_also_imports_its_entry() {
    let both = [
        row(0, "pkg", "pkg/__init__.py"),
        row(0, "pkg.mod", "pkg/mod.py"),
    ];
    assert_eq!(rows("main.py", "from pkg import mod\n"), both);
    // pkgx/ only starts with the same letters as pkg/.
    assert_eq!(rows("pkgx/a.py", "from pkg import mod\n"), both);
    assert_eq!(
        rows("pkg/a.py", "from pkg.sub import deep\n"),
        [
            row(0, "pkg.sub", "pkg/sub/__init__.py"),
            row(0, "pkg.sub.deep", "pkg/sub/deep.py")
        ]
    );
}

#[test]
fn a_member_that_is_not_a_module_keeps_the_entry() {
    assert_eq!(
        rows("pkg/a.py", "from pkg import name\n"),
        [row(0, "pkg", "pkg/__init__.py")]
    );
    assert_eq!(
        rows("pkg/a.py", "from pkg import mod, name\n"),
        [
            row(0, "pkg", "pkg/__init__.py"),
            row(0, "pkg.mod", "pkg/mod.py")
        ]
    );
    assert_eq!(
        rows("pkg/a.py", "from pkg import *\n"),
        [row(0, "pkg", "pkg/__init__.py")]
    );
}

#[test]
fn the_entry_itself_imports_the_submodules_it_names() {
    assert_eq!(
        rows("pkg/__init__.py", "from . import mod\nname = 1\n"),
        [row(0, "mod", "pkg/mod.py")]
    );
}

// The seaborn shape of U3 packet ar95: pkg/__init__.py imports pkg/widgets.py at load, and
// pkg/rcmod.py imports a sibling submodule with `from . import palettes`.
#[test]
fn a_sibling_import_inside_the_package_is_no_chain_through_the_entry() {
    let files = scanned(&[
        ("pkg/__init__.py", "from .widgets import show\n"),
        ("pkg/palettes.py", "x = 1\n"),
        ("pkg/rcmod.py", "from . import palettes\n"),
        (
            "pkg/widgets.py",
            "def show():\n    from .rcmod import set_style\n",
        ),
    ]);
    let scopes = Scopes::build(&files);
    let graph = SymbolGraph::imports(&scopes);
    let chain = import_chain(&graph, "pkg/rcmod.py", "pkg/widgets.py", |_, _| true);
    assert!(chain.is_none(), "{chain:?}");
}
