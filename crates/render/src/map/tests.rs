use app::{Companion, Companions, MapFile, MapResult};
use kernel::Bounded;

use super::map_text;

fn companion(path: &str, covers: &[&str], depth: u32) -> Companion {
    Companion {
        path: path.to_owned(),
        start: 3,
        end: 9,
        covers: covers.iter().map(|&cover| cover.to_owned()).collect(),
        depth,
        weight: 1.0,
    }
}

// One listed file; a depth-1 test that also names a cut seed, and a depth-2 test that names only
// the file it came through.
fn companion_map() -> MapResult {
    let rows = vec![
        companion("tests/test_a.py", &["a.py", "seed.py"], 1),
        companion("tests/test_b.py", &["helper.py"], 2),
    ];
    MapResult {
        files: vec![MapFile {
            path: "a.py".to_owned(),
            rank: 0.5,
            entries: Vec::new(),
            total: 0,
            id_prefix: "",
            reached: true,
            commits_window: None,
            last_commit_days: None,
        }],
        companions: Companions {
            rows: Bounded::cut(rows, 5),
            exhausted: false,
        },
        ..noted_map()
    }
}

// Every note this map can carry at once: no ranked file, an unsettled ranking, a budget that
// the rows pass, and a companion walk that hit its bound.
fn noted_map() -> MapResult {
    MapResult {
        files: Vec::new(),
        budget: 100,
        included: 0,
        candidates: 0,
        ranked: 0,
        rendered: 200,
        rank_converged: false,
        seeds: Vec::new(),
        unresolved_seeds: Vec::new(),
        skipped: Vec::new(),
        expand_order: Vec::new(),
        focus_order: Vec::new(),
        companions: Companions {
            rows: Bounded::cut(Vec::new(), 5),
            exhausted: true,
        },
    }
}

#[test]
fn every_note_line_of_a_map_starts_with_the_tool_marker() {
    assert_eq!(
        map_text(&noted_map()),
        "// note: the ranking behind this map did not converge, so the order of the files below \
         is a partial answer\n// note: no file in this repository parsed\n// note: this map \
         renders to 200 tokens against a 100-token budget: the 0 ranked files are listed whatever \
         the budget allows, and the budget bounds only the symbols shown under them\n\nno ranked \
         files\n\n// note: the walk out to these tests hit its node bound before it finished, so \
         this section is a floor rather than every test\n"
    );
}

#[test]
fn a_count_of_one_takes_the_singular_in_the_notes_and_the_companion_cut() {
    let mut map = companion_map();
    map.companions.rows = Bounded::cut(map.companions.rows.rows().to_vec(), 1);
    let text = map_text(&map);
    assert!(text.contains(
        "\n// note: this map renders to 200 tokens against a 100-token budget: the 1 ranked file \
         is listed whatever the budget allows, and the budget bounds only the symbols shown under \
         it\n"
    ));
    assert!(text.ends_with("\n... 1 more test file not listed (limit 1)\n"));
    let kept_none = MapResult {
        ranked: 1,
        ..noted_map()
    };
    assert!(
        map_text(&kept_none).contains("\n// note: --max-files kept none of the 1 ranked file\n")
    );
    let fit_none = MapResult {
        ranked: 2,
        candidates: 1,
        ..noted_map()
    };
    let note =
        "\n// note: the 100-token budget left room for none of the 1 symbol; raise --budget\n";
    assert!(map_text(&fit_none).contains(note));
}

#[test]
fn a_companion_row_marks_each_file_that_the_map_does_not_list() {
    let text = map_text(&companion_map());
    let section = &text[text.find("tests exercising").unwrap()..];
    assert_eq!(
        section,
        "tests exercising the files above:\n  tests/test_a.py:3-9  -- file references a.py, \
         seed.py (not listed)\n  tests/test_b.py:3-9  -- file references helper.py (not listed)\n"
    );
}
