// A committed sandbox repository opened for the graph operations' tests: the services, the
// repository, a snapshot with no git state, and a parsing fact source.

use git::Snapshot;
use store::FactSource;

use crate::testing::Sandbox;
use crate::{Reading, Repo, Services};

pub(crate) struct Scanned {
    _sandbox: Sandbox,
    pub(crate) services: Services,
    repo: Repo,
    snapshot: Snapshot,
    source: FactSource,
}

impl Scanned {
    pub(crate) fn new(files: &[(&str, &str)]) -> Scanned {
        let sandbox = Sandbox::new();
        let root = sandbox.repo("repo", files);
        let services = sandbox.services(&root);
        let loaded = Repo::load(&services, &root);
        Scanned {
            repo: loaded.unwrap_or_else(|error| panic!("{error}")),
            services,
            snapshot: Snapshot {
                head_sha: None,
                tree_oid: None,
                dirty_count: None,
                note: String::new(),
            },
            source: FactSource::parsing(),
            _sandbox: sandbox,
        }
    }

    pub(crate) fn reading(&self) -> Reading<'_> {
        Reading {
            repo: &self.repo,
            snapshot: &self.snapshot,
            facts: &self.source,
            deadline: None,
        }
    }
}

// Two modules that import each other's functions, and a third that references one by name.
pub(crate) const LINKED: [(&str, &str); 4] = [
    ("pkg/__init__.py", ""),
    (
        "pkg/money.py",
        "def format_money(amount):\n    return str(amount)\n\n\ndef unused_helper():\n    \
         return 1\n",
    ),
    (
        "pkg/report.py",
        "from pkg.money import format_money\n\n\ndef render(total):\n    return \
         format_money(total)\n",
    ),
    (
        "pkg/main.py",
        "from pkg.report import render\n\n\ndef main():\n    return render(3)\n",
    ),
];

// A helper, a module that calls it, and the helper's test.
pub(crate) const TESTED: [(&str, &str); 3] = [
    ("core.py", "def helper(value):\n    return value\n"),
    (
        "user.py",
        "from core import helper\n\n\ndef use(value):\n    return helper(value)\n",
    ),
    (
        "tests/test_core.py",
        "from core import helper\n\n\ndef test_helper():\n    helper(1)\n",
    ),
];
