//! The update of a kept graph: each edit of the corpus, applied to a scan of a characterization
//! fixture, leaves every kept part equal to a full build over the edited scan.

use std::collections::BTreeMap;
use std::path::Path;

use syntax::Language;

use super::equality::{Canonical, Parts, assert_same};
use super::rust::scanned;
use crate::{FileFacts, KeptParts, NameIndex, Rebuild, Resolver, Scopes, SymbolGraph, update};

// One change to one file of a scan: a text replaced in it, its whole new text, or its removal.
enum Change {
    Replace(&'static str, &'static str),
    Write(&'static str),
    Delete,
}

use Change::{Delete, Replace, Write};

// A scan and the series of edits that the kept parts are carried through, one update each. A
// step that `rebuilds` must hand its scan to a full build, and ends the series.
struct Case {
    name: &'static str,
    fixture: &'static str,
    extra: &'static [(&'static str, &'static str)],
    steps: &'static [&'static [(&'static str, Change)]],
    rebuilds: Option<Rebuild>,
}

// The texts of every scanned file of a fixture, by path, with the extra files of the case.
fn texts(case: &Case) -> BTreeMap<String, String> {
    let mut texts = BTreeMap::new();
    if !case.fixture.is_empty() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/characterization/fixtures");
        read_tree(&root.join(case.fixture), "", &mut texts);
    }
    for (path, text) in case.extra {
        texts.insert((*path).to_owned(), (*text).to_owned());
    }
    texts
}

fn read_tree(dir: &Path, prefix: &str, texts: &mut BTreeMap<String, String>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        if entry.path().is_dir() {
            read_tree(&entry.path(), &format!("{path}/"), texts);
        } else if Language::from_path(&path).is_some() {
            let text = std::fs::read_to_string(entry.path())
                .unwrap_or_else(|error| panic!("{path}: {error}"));
            texts.insert(path, text);
        }
    }
}

fn changed(texts: &mut BTreeMap<String, String>, edits: &[(&str, Change)], case: &str) {
    for (path, change) in edits {
        match change {
            Replace(from, to) => {
                let text = texts
                    .get_mut(*path)
                    .unwrap_or_else(|| panic!("{case}: no {path}"));
                assert!(text.contains(from), "{case}: {path} holds no {from:?}");
                *text = text.replacen(from, to, 1);
            }
            Write(text) => {
                texts.insert((*path).to_owned(), (*text).to_owned());
            }
            Delete => {
                texts
                    .remove(*path)
                    .unwrap_or_else(|| panic!("{case}: no {path}"));
            }
        }
    }
}

fn scan(texts: &BTreeMap<String, String>) -> Vec<FileFacts> {
    let pairs: Vec<(&str, &str)> = texts
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    scanned(&pairs)
}

// The list of a later scan: a file whose facts did not change keeps the facts of `before`, as a
// server's store keeps them.
fn carried<'a>(before: &[&'a FileFacts], after: &'a [FileFacts]) -> Vec<&'a FileFacts> {
    let same_paths = before.len() == after.len();
    let kept = |(at, file): (usize, &'a FileFacts)| match before.get(at) {
        Some(held) if same_paths && *held == file => *held,
        _ => file,
    };
    after.iter().enumerate().map(kept).collect()
}

fn run(case: &Case) {
    let mut texts = texts(case);
    let mut scans = vec![scan(&texts)];
    for step in case.steps {
        changed(&mut texts, step, case.name);
        scans.push(scan(&texts));
    }
    let mut lists: Vec<Vec<&FileFacts>> = vec![scans[0].iter().collect()];
    for after in &scans[1..] {
        let carried = carried(lists.last().expect("one list per scan"), after);
        lists.push(carried);
    }
    let mut index = NameIndex::build(&lists[0]);
    let mut scopes = Scopes::build(&lists[0]);
    let mut graph = SymbolGraph::build(&Resolver::new(&index, &scopes), None).expect("no deadline");
    for (step, pair) in lists.windows(2).enumerate() {
        let parts = KeptParts {
            index: Some(&mut index),
            scopes: Some(&mut scopes),
            graph: Some(&mut graph),
        };
        let updated = update(parts, &pair[0], &pair[1]);
        let last = step + 1 == case.steps.len();
        match (case.rebuilds, updated) {
            (Some(rebuild), Err(found)) if last => {
                assert_eq!(found, rebuild, "{}: step {step}", case.name);
                return;
            }
            (_, Err(found)) => panic!("{}: step {step} rebuilds: {found:?}", case.name),
            (Some(rebuild), Ok(_)) if last => {
                panic!("{}: step {step} updates, not {rebuild:?}", case.name)
            }
            (_, Ok(_)) => {}
        }
        let kept = Canonical::of(&Parts {
            files: &pair[1],
            index: Some(&index),
            scopes: Some(&scopes),
            graph: Some(&graph),
            ..Parts::default()
        });
        let label = format!("{}: step {step}", case.name);
        assert_same(&label, &kept, &Canonical::built(&pair[1]));
    }
}

// A package whose entry re-exports a name, a module that the entry does not bind, and a file that
// star-imports a module.
const PACKAGE: [(&str, &str); 5] = [
    ("pkg/__init__.py", "from pkg.core import run\n"),
    (
        "pkg/core.py",
        "def run():\n    return 1\n\n\ndef other():\n    return 2\n",
    ),
    ("pkg/mod.py", "x = 1\n"),
    (
        "pkg/user.py",
        "from pkg import mod, run\n\n\ndef use():\n    return run() + mod.x\n",
    ),
    (
        "app.py",
        "from pkg import run\nfrom pkg.core import *\n\n\ndef main():\n    return run() + other()\n",
    ),
];

// A name with two definitions in each of two language families, spelled ambiguously in a file of
// each; the Python file comes first in scan order.
const FAMILIES: [(&str, &str); 6] = [
    ("a.py", "def helper():\n    return 1\n"),
    ("b.py", "def helper():\n    return 2\n"),
    ("c.py", "def run():\n    return helper() + helper()\n"),
    ("d.js", "function run() { return helper(); }\n"),
    ("x.js", "function helper() { return 1; }\n"),
    ("y.js", "function helper() { return 2; }\n"),
];

const CASES: &[Case] = &[
    Case {
        name: "a body edit",
        fixture: "repo_nested",
        extra: &[],
        steps: &[&[(
            "billing/invoice.py",
            Replace(
                "return tax_on(self.net, self.band)",
                "return tax_on(self.band, self.net)",
            ),
        )]],
        rebuilds: None,
    },
    Case {
        name: "a body edit that moves the later symbols",
        fixture: "repo_nested",
        extra: &[],
        steps: &[&[(
            "billing/invoice.py",
            Replace(
                "    def tax(self) -> Decimal:\n",
                "    def tax(self) -> Decimal:\n\n\n\n",
            ),
        )]],
        rebuilds: None,
    },
    Case {
        name: "a definition added, renamed, reparented and removed",
        fixture: "repo_nested",
        extra: &[],
        steps: &[
            &[(
                "billing/money.py",
                Replace(
                    "def sum_amounts(",
                    "def zero_amount() -> Decimal:\n    return Decimal(\"0\")\n\n\ndef sum_amounts(",
                ),
            )],
            &[(
                "billing/money.py",
                Replace("def zero_amount()", "def nil_amount()"),
            )],
            &[(
                "billing/money.py",
                Replace(
                    "def nil_amount() -> Decimal:\n    return",
                    "class Nil:\n    def nil_amount(self) -> Decimal:\n        return",
                ),
            )],
            &[(
                "billing/money.py",
                Replace(
                    "class Nil:\n    def nil_amount(self) -> Decimal:\n        return Decimal(\"0\")\n\n\n",
                    "",
                ),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "a name that enters the index and leaves it",
        fixture: "repo_go",
        extra: &[],
        steps: &[
            &[(
                "pricing.go",
                Write("package main\n\nfunc onlyHere() int { return zzUnheardOf() }\n"),
            )],
            &[(
                "pricing.go",
                Write("package main\n\nfunc onlyHere() int { return 1 }\n"),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "a duplicate name that gains an ordinal",
        fixture: "repo_py",
        extra: &[("dup.py", "def twice():\n    return 1\n")],
        steps: &[&[(
            "dup.py",
            Write("def twice():\n    return 1\n\n\ndef twice():\n    return 2\n"),
        )]],
        rebuilds: None,
    },
    Case {
        name: "a data file",
        fixture: "tier2",
        extra: &[],
        steps: &[&[("config.json", Replace("{", "{\"added\": 1,"))]],
        rebuilds: None,
    },
    Case {
        name: "a Rust body edit and a line that moves a use declaration",
        fixture: "callee_overlap_rust/base",
        extra: &[],
        steps: &[
            &[(
                "src/ledger.rs",
                Replace("let book = ", "let book =\n        "),
            )],
            &[(
                "src/ledger.rs",
                Replace("use crate::helpers", "// moved\n\nuse crate::helpers"),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "an import added and removed",
        fixture: "repo_nested",
        extra: &[],
        steps: &[
            &[(
                "billing/invoice.py",
                Replace(
                    "from billing.tax import tax_on\n",
                    "from billing.tax import tax_on\nfrom billing.money import round_amount\n",
                ),
            )],
            &[(
                "billing/invoice.py",
                Replace("from billing.money import round_amount\n", ""),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "a package entry's re-export, an entry name that changes what a module loads, and a star import",
        fixture: "",
        extra: &PACKAGE,
        steps: &[
            &[(
                "pkg/__init__.py",
                Write("from pkg.core import run, other\n"),
            )],
            &[(
                "pkg/__init__.py",
                Write("from pkg.core import run, other\nmod = 1\n"),
            )],
            &[(
                "pkg/core.py",
                Replace(
                    "def other():",
                    "def third():\n    return 3\n\n\ndef other():",
                ),
            )],
            &[("app.py", Replace("from pkg.core import *\n", ""))],
        ],
        rebuilds: None,
    },
    Case {
        name: "a JavaScript import through an index file",
        fixture: "",
        extra: &[
            (
                "lib/index.ts",
                "export function helper(): number { return 1; }\n",
            ),
            (
                "lib/other.ts",
                "export function helper(): number { return 2; }\n",
            ),
            (
                "main.ts",
                "import { helper } from './lib';\n\nexport function main(): number { return helper(); }\n",
            ),
        ],
        steps: &[
            &[("main.ts", Replace("from './lib'", "from './lib/other'"))],
            &[(
                "lib/other.ts",
                Replace("function helper()", "function aide()"),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "a Java import",
        fixture: "",
        extra: &[
            (
                "src/a/b/Money.java",
                "package a.b;\npublic class Money { public int cents() { return 1; } }\n",
            ),
            (
                "src/a/c/Report.java",
                "package a.c;\nimport a.b.Money;\npublic class Report { int run(Money m) { return m.cents(); } }\n",
            ),
        ],
        steps: &[
            &[("src/a/c/Report.java", Replace("import a.b.Money;\n", ""))],
            &[(
                "src/a/b/Money.java",
                Replace(
                    "public int cents()",
                    "public int pence() { return 2; }\n  public int cents()",
                ),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "a C include",
        fixture: "",
        extra: &[
            ("util.h", "int helper(int x);\n"),
            (
                "util.c",
                "#include \"util.h\"\nint helper(int x) { return x; }\n",
            ),
            (
                "main.c",
                "#include \"util.h\"\nint main(void) { return helper(1); }\n",
            ),
        ],
        steps: &[
            &[("util.h", Write("int helper(int x);\nint other(int x);\n"))],
            &[("main.c", Replace("#include \"util.h\"\n", ""))],
        ],
        rebuilds: None,
    },
    Case {
        name: "a Go name of the same package",
        fixture: "repo_go",
        extra: &[],
        steps: &[
            &[(
                "inventory.go",
                Replace(
                    "func NewInventory(",
                    "func FormatMoney(amount float64) string { return \"\" }\n\nfunc NewInventory(",
                ),
            )],
            &[(
                "pricing.go",
                Replace("func FormatMoney(", "func formatMoneyOld("),
            )],
        ],
        rebuilds: None,
    },
    Case {
        name: "the first ambiguous spelling of a name moves to another file and family, and back",
        fixture: "",
        extra: &FAMILIES,
        steps: &[
            &[(
                "c.py",
                Replace("    return helper() + helper()\n", "    return 0\n"),
            )],
            &[("c.py", Replace("    return 0\n", "    return helper()\n"))],
            &[("d.js", Replace("helper()", "0"))],
        ],
        rebuilds: None,
    },
    Case {
        name: "two definitions that a chain reaches come to share a line",
        fixture: "",
        extra: &[
            ("pkg/core.py", "A = 1\nB = 2\n"),
            (
                "app.py",
                "from pkg.core import A as N\nfrom pkg.core import B as N\n\n\ndef f():\n    return N\n",
            ),
        ],
        steps: &[
            &[("pkg/core.py", Write("A = B = 1\n"))],
            &[("pkg/core.py", Write("\nA = 1\nB = 2\n"))],
        ],
        rebuilds: None,
    },
    Case {
        name: "a file added",
        fixture: "repo_py",
        extra: &[],
        steps: &[&[("added.py", Write("def added():\n    return 1\n"))]],
        rebuilds: Some(Rebuild::Paths),
    },
    Case {
        name: "a file deleted",
        fixture: "repo_py",
        extra: &[],
        steps: &[&[("reports.py", Delete)]],
        rebuilds: Some(Rebuild::Paths),
    },
    Case {
        name: "a file renamed",
        fixture: "repo_go",
        extra: &[(
            "moved.go",
            "package main\n\nfunc moved() int { return 1 }\n",
        )],
        steps: &[&[
            ("moved.go", Delete),
            (
                "renamed.go",
                Write("package main\n\nfunc moved() int { return 1 }\n"),
            ),
        ]],
        rebuilds: Some(Rebuild::Paths),
    },
    Case {
        name: "a Cargo manifest's package",
        fixture: "callee_overlap_rust/base",
        extra: &[],
        steps: &[&[(
            "Cargo.toml",
            Replace("name = \"planted\"", "name = \"renamed\""),
        )]],
        rebuilds: Some(Rebuild::Manifest),
    },
    Case {
        name: "a Rust use declaration",
        fixture: "callee_overlap_rust/base",
        extra: &[],
        steps: &[&[(
            "src/ledger.rs",
            Replace(
                "use crate::helpers",
                "use crate::invoice::Invoice;\nuse crate::helpers",
            ),
        )]],
        rebuilds: Some(Rebuild::RustModules),
    },
    Case {
        name: "a Rust mod item",
        fixture: "callee_overlap_rust/base",
        extra: &[],
        steps: &[&[(
            "src/lib.rs",
            Replace("pub mod builders;", "pub mod added;\npub mod builders;"),
        )]],
        rebuilds: Some(Rebuild::RustModules),
    },
];

#[test]
fn each_edit_of_the_corpus_leaves_the_parts_equal_to_a_full_build() {
    for case in CASES {
        run(case);
    }
}
