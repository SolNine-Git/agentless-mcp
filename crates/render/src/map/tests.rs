use app::{Companions, MapResult};
use kernel::Bounded;

use super::map_text;

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
         is a partial answer\n// note: nothing in this repository parsed into symbols\n// note: \
         this map renders to 200 tokens against a 100-token budget: every one of the 0 ranked \
         files is listed whatever the budget allows, and the budget bounds only the symbols shown \
         under them\n\nno ranked files\n\n// note: the walk out to these tests hit its node bound \
         before it finished, so this section is a floor rather than every test\n"
    );
}
