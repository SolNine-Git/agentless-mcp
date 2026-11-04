use std::collections::BTreeSet;

use super::{BodyMapResult, body_map};
use crate::endpoints::scanned::Scanned;
use crate::{IntArg, MapRequest};

// A decorator factory: each function but the last is nested in the one before it.
const DECO: &str = "\
def deconstructible(*args, path=None):
    def decorator(klass):
        def __new__(cls, *args, **kwargs):
            obj = super(klass, cls).__new__(cls)
            return obj

        def deconstruct(obj):
            return (path, obj)

        klass.__new__ = staticmethod(__new__)
        klass.deconstruct = deconstruct
        return klass

    if not args:
        return decorator
    return decorator(*args)
";

const USER: &str = "\
from deco import deconstructible


@deconstructible
class Thing:
    def size(self):
        return 1
";

fn mapped() -> BodyMapResult {
    let scanned = Scanned::new(&[("deco.py", DECO), ("user.py", USER)]);
    let request = MapRequest {
        focus: vec!["deco.py".to_owned()],
        budget: IntArg::parse("1500"),
        max_files: None,
        granularity: None,
    };
    let outcome = body_map(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn no_source_line_prints_twice_in_one_file() {
    let result = mapped();
    let mut printed = BTreeSet::new();
    for card in &result.bodies.cards {
        for line in card.start_line..=card.end_line {
            let first = printed.insert((card.path.as_str(), line));
            assert!(first, "{}:{line} prints twice", card.path);
        }
    }
    let focused = result
        .bodies
        .cards
        .iter()
        .filter(|card| card.path == "deco.py");
    assert!(focused.count() > 0, "the focus file shows no body");
}

#[test]
fn a_file_counts_only_the_symbols_that_show_no_body() {
    let result = mapped();
    let deco = (result.map.files.iter())
        .find(|file| file.path == "deco.py")
        .unwrap_or_else(|| panic!("deco.py is not ranked"));
    let shown = (result.bodies.cards.iter())
        .filter(|card| card.path == "deco.py")
        .count();
    assert_eq!(deco.omitted(), 4 - shown);
}
