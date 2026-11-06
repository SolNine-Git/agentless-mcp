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
    rows_of(&[(path, text)], path)
}

// The rows of the file `path` when each `(path, text)` of `written` replaces or adds a file.
fn rows_of(written: &[(&str, &str)], path: &str) -> Vec<(usize, String, String)> {
    let mut files = PACKAGE.to_vec();
    files.retain(|(held, _)| written.iter().all(|(replaced, _)| replaced != held));
    files.extend_from_slice(written);
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

// The rows of `from pkg import mod` in pkg/a.py when pkg/__init__.py is written as `entry`.
fn entry_rows(entry: &str) -> Vec<(usize, String, String)> {
    let written = [
        ("pkg/__init__.py", entry),
        ("pkg/a.py", "from pkg import mod\n"),
    ];
    rows_of(&written, "pkg/a.py")
}

fn kept() -> [(usize, String, String); 2] {
    [
        row(0, "pkg", "pkg/__init__.py"),
        row(0, "pkg.mod", "pkg/mod.py"),
    ]
}

fn dropped() -> [(usize, String, String); 1] {
    [row(0, "pkg.mod", "pkg/mod.py")]
}

#[test]
fn an_entry_that_defines_the_member_keeps_the_row_to_it() {
    let entries = [
        "def mod():\n    pass\n",
        "class mod:\n    pass\n",
        "if True:\n    def mod():\n        pass\n",
    ];
    for entry in entries {
        assert_eq!(entry_rows(entry), kept(), "{entry:?}");
    }
}

// Only an upper-case assignment is a symbol of the facts, so the rule never sees a lower-case one.
#[test]
fn an_entry_that_assigns_an_upper_case_member_keeps_the_row_to_it() {
    let written = [
        ("pkg/__init__.py", "MOD = 1\n"),
        ("pkg/MOD.py", ""),
        ("pkg/a.py", "from pkg import MOD\n"),
    ];
    assert_eq!(
        rows_of(&written, "pkg/a.py"),
        [
            row(0, "pkg", "pkg/__init__.py"),
            row(0, "pkg.MOD", "pkg/MOD.py")
        ]
    );
    assert_eq!(entry_rows("mod = 1\n"), dropped());
}

#[test]
fn an_entry_that_imports_another_object_as_the_member_keeps_the_row_to_it() {
    let entries = [
        "from .mod import mod\n",
        "from pkg.mod import mod\n",
        "from .sub import deep as mod\n",
        "from . import sub as mod\n",
        "import json as mod\n",
        "try:\n    from .mod import mod\nexcept ImportError:\n    pass\n",
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    pass\nelse:\n    from .mod import mod\n",
    ];
    for entry in entries {
        assert_eq!(entry_rows(entry), kept(), "{entry:?}");
    }
}

#[test]
fn an_entry_that_binds_the_submodule_itself_or_another_name_gives_no_row_to_it() {
    let entries = [
        "from . import mod\n",
        "from pkg import mod\n",
        "import pkg.mod as mod\n",
        "from .mod import y\nname = 1\n",
        "",
    ];
    for entry in entries {
        assert_eq!(entry_rows(entry), dropped(), "{entry:?}");
    }
}

#[test]
fn an_import_that_does_not_run_at_load_rebinds_no_member() {
    let entries = [
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from .mod import mod\n",
        "import typing as t\nif t.TYPE_CHECKING:\n    from .mod import mod\n",
        "def load():\n    from .mod import mod\n",
    ];
    for entry in entries {
        assert_eq!(entry_rows(entry), dropped(), "{entry:?}");
    }
}

#[test]
fn one_rebound_member_keeps_the_row_of_the_whole_statement() {
    let entry = ("pkg/__init__.py", "from .mod import mod\n");
    let mixed = [entry, ("pkg/a.py", "from pkg import mod, sub\n")];
    assert_eq!(
        rows_of(&mixed, "pkg/a.py"),
        [
            row(0, "pkg", "pkg/__init__.py"),
            row(0, "pkg.mod", "pkg/mod.py"),
            row(0, "pkg.sub", "pkg/sub/__init__.py")
        ]
    );
    // The entry's attribute is the member's name, not the importer's alias for it.
    let aliased = [entry, ("pkg/a.py", "from pkg import mod as m\n")];
    assert_eq!(rows_of(&aliased, "pkg/a.py"), kept());
    let nested = [entry, ("pkg/sub/b.py", "from .. import mod\n")];
    assert_eq!(
        rows_of(&nested, "pkg/sub/b.py"),
        [row(0, "", "pkg/__init__.py"), row(0, "mod", "pkg/mod.py")]
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
