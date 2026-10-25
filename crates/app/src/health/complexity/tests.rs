use std::collections::BTreeMap;

use kernel::Key;
use metrics::{FileMetrics, Unit};
use store::Served;
use syntax::Language;

use super::{PlacedUnit, Site, Sites, Target, unit_stable_id, walked};
use crate::Skipped;

fn target(path: &str) -> Target {
    Target {
        path: path.to_owned(),
        language: Language::Python,
        skipped: None,
    }
}

fn not_measured(path: &str) -> Served<Result<FileMetrics, Skipped>> {
    let skipped = Skipped {
        path: path.to_owned(),
        reason: "unreadable".to_owned(),
    };
    Served {
        value: Err(skipped),
        warning: None,
    }
}

#[test]
fn the_first_file_the_budget_turned_away_ends_the_walk() {
    let targets = ["a.py", "b.py", "c.py", "d.py"].map(target);
    let measured = vec![
        Ok(Some(not_measured("a.py"))),
        Ok(Some(not_measured("b.py"))),
        Ok(None),
        Ok(Some(not_measured("d.py"))),
    ];

    let walk = (walked(&targets, measured).value).expect("no deadline stops it");

    let paths: Vec<&str> = (walk.not_measured.iter())
        .map(|skipped| skipped.path.as_str())
        .collect();
    assert_eq!(paths, ["a.py", "b.py"]);
    assert_eq!((walk.walked, walk.in_scope), (2, 4));
}

#[test]
fn a_walk_the_budget_never_cut_keeps_every_file() {
    let targets = ["a.py", "b.py"].map(target);
    let measured = vec![
        Ok(Some(not_measured("a.py"))),
        Ok(Some(not_measured("b.py"))),
    ];

    let walk = (walked(&targets, measured).value).expect("no deadline stops it");

    assert_eq!((walk.walked, walk.in_scope), (2, 2));
}

fn placed(qualname: &str, start_line: u32) -> PlacedUnit {
    let unit = Unit {
        qualname: qualname.to_owned(),
        start_byte: 0,
        end_byte: 0,
        start_line,
        body_key: Key::new([0; 32]),
        cc: 1,
        sloc: 1,
    };
    PlacedUnit {
        path: "a.py".to_owned(),
        unit,
    }
}

fn site<'d>(stable_id: &'d str, name: &'d str, (line, end_line): (u32, u32)) -> Site<'d> {
    Site {
        line,
        end_line,
        stable_id,
        name,
    }
}

#[test]
fn a_unit_takes_the_innermost_callable_of_its_name_whose_span_holds_its_start() {
    let sites: Sites<'_> = BTreeMap::from([(
        "a.py",
        vec![
            site("py:a.py::top", "top", (4, 9)),
            site("py:a.py::Point.make", "make", (20, 30)),
            site("py:a.py::Point.make.wrapped", "wrapped", (26, 28)),
            site("py:a.py::twin", "twin", (40, 41)),
            site("py:a.py::twin#2", "twin", (40, 42)),
        ],
    )]);
    let id = |qualname, start_line| unit_stable_id(&placed(qualname, start_line), &sites);
    assert_eq!(id("top", 8).as_deref(), Some("py:a.py::top"));
    assert_eq!(id("Point.make", 22).as_deref(), Some("py:a.py::Point.make"));
    assert_eq!(
        id("Point.make.wrapped", 27).as_deref(),
        Some("py:a.py::Point.make.wrapped")
    );
    assert_eq!(id("top", 12), None);
    assert_eq!(id("twin", 40), None);
}
