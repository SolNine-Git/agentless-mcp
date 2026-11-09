use app::search::{FileCount, Matches, Noise, NoiseClass, SearchRequest, Searched};
use app::{IntArg, Skipped};

use crate::{SearchAnswer, search_text};

fn counted(count: usize) -> Vec<FileCount> {
    (1..=count)
        .map(|at| FileCount {
            path: format!("vendor/m{at}.py"),
            count: 1,
        })
        .collect()
}

#[test]
fn a_count_line_names_five_files_and_counts_the_rest() {
    let matches = Matches {
        matches: 7,
        files: 7,
        searched: 9,
        groups: Vec::new(),
        repeats: Vec::new(),
        shown: 0,
        rest: Vec::new(),
        noise: vec![Noise {
            class: NoiseClass::Vendored,
            matches: 7,
            files: counted(7),
        }],
        skipped: vec![Skipped {
            path: "big.py".to_owned(),
            reason: "skipped: 1000001 bytes exceeds the per-file cap of 1000000 bytes".to_owned(),
        }],
    };
    let request = SearchRequest {
        pattern: Some("run".to_owned()),
        files: None,
        path: None,
        ignore_case: false,
        context: IntArg::from_usize(0),
        limit: IntArg::from_usize(100),
    };
    let searched = Searched::Matches(matches);
    let text = search_text(&SearchAnswer {
        request: &request,
        searched: &searched,
    });
    let expected = "search /run/: 7 matches in 7 files\n+ 7 matches in 7 vendored files: \
                    vendor/m1.py (1), vendor/m2.py (1), vendor/m3.py (1), vendor/m4.py (1), \
                    vendor/m5.py (1) and 2 more files. path=<file> shows them.\n// warning: 1 \
                    file was not searched: big.py (skipped: 1000001 bytes exceeds the per-file \
                    cap of 1000000 bytes)\n";
    assert_eq!(text, expected);
}
