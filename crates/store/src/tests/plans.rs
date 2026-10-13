use std::collections::BTreeMap;

use kernel::Key;
use syntax::Unparsable;

use super::indexed::{PYTHON, file, observed};
use crate::plan::Entry;
use crate::{
    Analysis, IndexFailure, IndexSkip, Observed, Plan, Previous, Read, analysis, analyze, plan,
    settle,
};

const OTHER: &str = "x = 1\n";

// A previous run that recorded `files` under `analysis`, each measured as the index measures it.
fn previous(files: &[(&str, &str)], analysis: Key) -> Previous {
    let entries: BTreeMap<String, Entry> = observed(files)
        .into_iter()
        .map(|found| {
            let Read::Text(text) = &found.read else {
                unreachable!("observed files are read whole")
            };
            let entry = Entry {
                language: found.language.name().to_owned(),
                digest: text.digest().bytes().to_vec(),
                measured: found.measure,
            };
            (found.path, entry)
        })
        .collect();
    Previous {
        entries,
        analysis: Some(analysis.bytes().to_vec()),
    }
}

fn planned(plan: &Plan) -> Vec<&str> {
    plan.extract
        .iter()
        .map(|planned| planned.file.path.as_str())
        .collect()
}

fn parsed(plan: &Plan) -> Vec<Result<Analysis, Unparsable>> {
    plan.extract
        .iter()
        .map(|planned| analyze(&planned.file, planned.measure))
        .collect()
}

// One row of the reuse table: the inputs of a plan and the count of files it extracts.
struct ReuseRow<'a> {
    name: &'a str,
    previous: &'a Previous,
    observed: Vec<Observed>,
    analysis: Key,
    force: bool,
    extracted: usize,
}

#[test]
fn each_reuse_condition_is_one_row_of_the_table() {
    let recorded = previous(&[("a.py", PYTHON)], analysis());
    let same = || observed(&[("a.py", PYTHON)]);
    let mut flipped = same();
    flipped[0].measure = !flipped[0].measure;
    let other_analysis = Key::new([7; 32]);
    let empty = Previous::default();
    let row = |name, previous, observed, analysis, force, extracted| ReuseRow {
        name,
        previous,
        observed,
        analysis,
        force,
        extracted,
    };
    let rows = [
        row("unchanged", &recorded, same(), analysis(), false, 0),
        row("forced", &recorded, same(), analysis(), true, 1),
        row(
            "edited",
            &recorded,
            observed(&[("a.py", OTHER)]),
            analysis(),
            false,
            1,
        ),
        row("measure flipped", &recorded, flipped, analysis(), false, 1),
        row(
            "other analysis",
            &recorded,
            same(),
            other_analysis,
            false,
            1,
        ),
        row("new file", &empty, same(), analysis(), false, 1),
    ];
    for ReuseRow {
        name,
        previous,
        observed,
        analysis,
        force,
        extracted,
    } in rows
    {
        let plan = plan(previous, observed, analysis, force);
        assert_eq!(plan.extract.len(), extracted, "{name}");
        let results = parsed(&plan);
        assert_eq!(settle(plan, results).reused, 1 - extracted, "{name}");
    }
}

#[test]
fn a_file_of_another_language_at_the_same_path_is_extracted() {
    let mut recorded = previous(&[("a.py", PYTHON)], analysis());
    recorded
        .entries
        .get_mut("a.py")
        .expect("a recorded row")
        .language = "go".to_owned();

    let plan = plan(&recorded, observed(&[("a.py", PYTHON)]), analysis(), false);

    assert_eq!(planned(&plan), ["a.py"]);
}

// An oversize file is skipped and an unreadable one fails; neither is seen, so each recorded row
// of theirs goes. A recorded file that left the repository is pruned and goes too.
#[test]
fn files_not_read_whole_lose_their_rows_and_a_vanished_file_is_pruned() {
    let recorded = previous(
        &[
            ("big.py", OTHER),
            ("gone.py", OTHER),
            ("locked.py", OTHER),
            ("kept.py", OTHER),
        ],
        analysis(),
    );
    let mut walked = observed(&[("big.py", OTHER), ("kept.py", OTHER), ("locked.py", OTHER)]);
    walked[0].read = Read::Oversize { size: 2_000_000 };
    walked[2].read = Read::Unreadable {
        reason: "unreadable: Permission denied".to_owned(),
    };

    let plan = plan(&recorded, walked, analysis(), false);
    assert!(plan.extract.is_empty());
    let settled = settle(plan, Vec::new());

    assert_eq!(settled.delete, ["big.py", "gone.py", "locked.py"]);
    assert_eq!((settled.reused, settled.pruned), (1, 1));
    let skip = IndexSkip {
        path: "big.py".to_owned(),
        reason: "skipped: 2000000 bytes exceeds the per-file cap of 1000000 bytes".to_owned(),
    };
    assert_eq!(settled.skips, [skip]);
    let failure = IndexFailure {
        path: "locked.py".to_owned(),
        reason: "unreadable: Permission denied".to_owned(),
    };
    assert_eq!(settled.failures, [failure]);
}

// A parse failure is listed at its walk position among the read failures, and the file's old row
// goes. Each extracted file that was recorded is deleted before its new row is written.
#[test]
fn a_parse_failure_takes_its_walk_place_and_its_old_row() {
    let recorded = previous(&[("a.py", OTHER), ("c.py", OTHER)], analysis());
    let mut walked = observed(&[("a.py", PYTHON), ("b.py", OTHER), ("c.py", PYTHON)]);
    walked[1].read = Read::Unreadable {
        reason: "unreadable: Permission denied".to_owned(),
    };
    let plan = plan(&recorded, walked, analysis(), false);
    assert_eq!(planned(&plan), ["a.py", "c.py"]);
    let mut results = parsed(&plan);
    results[0] = Err(Unparsable::NoTree {
        language: syntax::Language::Python,
    });

    let settled = settle(plan, results);

    let failed: Vec<&str> = settled
        .failures
        .iter()
        .map(|failure| failure.path.as_str())
        .collect();
    assert_eq!(failed, ["a.py", "b.py"]);
    assert_eq!(settled.delete, ["a.py", "c.py"]);
    let written: Vec<&str> = settled
        .rows
        .iter()
        .map(|row| row.file.path.as_str())
        .collect();
    assert_eq!(written, ["c.py"]);
}

#[test]
#[should_panic(expected = "one parse result for each planned file")]
fn a_parse_list_of_another_length_is_a_defect() {
    let plan = plan(
        &Previous::default(),
        observed(&[("a.py", PYTHON)]),
        analysis(),
        false,
    );
    drop(settle(plan, Vec::new()));
}

#[test]
fn the_extracted_files_keep_the_walk_order_and_their_measure() {
    let walked = observed(&[("b.py", PYTHON), ("a.js", OTHER), ("c.md.py", PYTHON)]);
    let measures: Vec<bool> = walked.iter().map(|found| found.measure).collect();

    let plan = plan(&Previous::default(), walked, analysis(), false);

    assert_eq!(planned(&plan), ["b.py", "a.js", "c.md.py"]);
    let kept: Vec<bool> = plan.extract.iter().map(|planned| planned.measure).collect();
    assert_eq!(kept, measures);
    assert_eq!(plan.extract[0].file, file("b.py", PYTHON));
}
