use std::sync::Arc;

use syntax::Language;

use super::cases::{BUSY, ROOT, Scratch, cases, source_text, usable};
use crate::Lookup;
use crate::open::open;

// The indexer stored what the metrics crate measures now: a hit holds the measured blocks, and an
// unmeasured file does not measure.
#[test]
fn the_stored_blocks_equal_the_blocks_that_metrics_measures() {
    let scratch = Scratch::new();
    let found = usable(open(&scratch.database, ROOT, BUSY), "fixture");
    let plain = cases()
        .lookups
        .into_iter()
        .filter(|case| case.name == case.path);
    for case in plain {
        let language = Language::from_name(&case.language).expect("a language name");
        let text = source_text(&case.path, "none");
        let parsed = syntax::parse(language, Arc::from(text.as_str())).expect("the file parses");
        let measured = metrics::measure(&parsed);
        match found.blocks_for(&case.path, &text, language) {
            Lookup::Hit(blocks) => {
                let measured = measured.unwrap_or_else(|error| panic!("{}: {error}", case.path));
                assert_eq!(blocks, measured.blocks, "{}", case.path);
            }
            Lookup::Unmeasured(_) => assert!(measured.is_err(), "{}", case.path),
            other => panic!("{}: the fixture gives {other:?}", case.path),
        }
    }
}
