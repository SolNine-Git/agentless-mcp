use metrics::FileMetrics;
use store::Served;
use syntax::Language;

use super::{Target, walked};
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
        Some(not_measured("a.py")),
        Some(not_measured("b.py")),
        None,
        Some(not_measured("d.py")),
    ];

    let walk = walked(&targets, measured).value;

    let paths: Vec<&str> = (walk.not_measured.iter())
        .map(|skipped| skipped.path.as_str())
        .collect();
    assert_eq!(paths, ["a.py", "b.py"]);
    assert_eq!((walk.walked, walk.in_scope), (2, 4));
}

#[test]
fn a_walk_the_budget_never_cut_keeps_every_file() {
    let targets = ["a.py", "b.py"].map(target);
    let measured = vec![Some(not_measured("a.py")), Some(not_measured("b.py"))];

    let walk = walked(&targets, measured).value;

    assert_eq!((walk.walked, walk.in_scope), (2, 2));
}
