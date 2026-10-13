// Warm equals cold: after any sequence of edits and index runs, a read through agentless.db gives
// each file what one parse of its text gives.

use std::path::Path;

use store::FileText;
use syntax::Language;

use super::{PYTHON, SCRIPT, indexed, services, walked};
use crate::testing::Sandbox;
use crate::{Repo, Services, Session};

const PATHS: [&str; 6] = ["a.py", "pkg/b.py", "c.js", "d.go", "tests/e.py", "f.json"];
const TEXTS: [&[u8]; 8] = [
    PYTHON.as_bytes(),
    SCRIPT.as_bytes(),
    b"package p\n\nfunc F() {}\n",
    b"{\"k\": [1, 2]}\n",
    b"def broken(:\n    pass\n",
    b"x = '\xff\xfe'\n",
    b"",
    b"class A:\n    def f(self):\n        return A\n",
];
const SEEDS: [u64; 4] = [1, 7, 4242, 0x5eed];
const STEPS: usize = 24;

// A linear congruential generator: the same edits on every run, with no dependency.
struct Draws(u64);

impl Draws {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bound = u64::try_from(bound).expect("a small bound");
        usize::try_from((self.0 >> 33) % bound).expect("a value below a small bound")
    }
}

#[test]
fn warm_equals_cold_over_random_edits_and_runs() {
    for seed in SEEDS {
        let sandbox = Sandbox::new();
        let root = sandbox.mkdir("tree");
        let services = services(&sandbox);
        let mut draws = Draws(seed);
        for step in 0..STEPS {
            match draws.below(8) {
                0..=4 => {
                    let path = PATHS[draws.below(PATHS.len())];
                    sandbox.put(&format!("tree/{path}"), TEXTS[draws.below(TEXTS.len())]);
                }
                5 => {
                    let path = PATHS[draws.below(PATHS.len())];
                    io::remove_file(&root.join(path)).expect("the remove succeeds");
                }
                _ => {
                    indexed(&services, &root, draws.below(4) == 0);
                    warm_equals_cold(&services, &root, &format!("seed {seed} step {step}"));
                }
            }
        }
    }
}

fn warm_equals_cold(services: &Services, root: &Path, case: &str) {
    let repo = Repo::load(services, root).expect("the repository loads");
    let session = Session::open_facts(services, repo, false).session.value;
    let session = session.expect("a session");
    let files = walked(services, root);
    let counts = session.cache.counts.as_ref().expect("the counts read");
    assert_eq!(
        counts.files,
        u64::try_from(files.len()).expect("a count"),
        "{case}"
    );
    for path in files {
        let bytes = io::read_bounded(&root.join(&path)).expect("the file reads");
        let language = Language::from_path(&path).expect("a known language");
        let file = FileText::decode(path.clone(), language, &bytes);
        let cold = store::analyze(&file, true);
        let symbols = session.facts.symbols(&file);
        let facts = session.facts.facts(&file);
        let metrics = session.facts.metrics(&file);
        let warnings = [&symbols.warning, &facts.warning, &metrics.warning];
        assert!(
            warnings.iter().all(|warning| warning.is_none()),
            "{case} {path}"
        );
        let cold_facts = cold.as_ref().map(|analysis| &analysis.facts);
        assert_eq!(facts.value.as_ref(), cold_facts, "{case} {path}");
        let cold_symbols = cold_facts.map(|facts| &facts.symbols);
        assert_eq!(symbols.value.as_ref(), cold_symbols, "{case} {path}");
        let cold_metrics = cold
            .as_ref()
            .ok()
            .and_then(|analysis| analysis.metrics.as_ref());
        assert_eq!(Some(&metrics.value), cold_metrics, "{case} {path}");
    }
}
