use super::{
    Matches, Measure, NoiseClass, PatternFault, Place, SearchError, SearchRequest, Searched,
    literal_files, search,
};
use crate::IntArg;
use crate::endpoints::scanned::Scanned;

fn request(pattern: Option<&str>) -> SearchRequest {
    SearchRequest {
        pattern: pattern.map(str::to_owned),
        files: None,
        path: None,
        ignore_case: false,
        context: IntArg::from_usize(0),
        limit: IntArg::from_usize(100),
    }
}

fn answered(files: &[(&str, &str)], request: &SearchRequest) -> Result<Searched, SearchError> {
    let scanned = Scanned::new(files);
    search(&scanned.services, &scanned.reading(), request).value
}

fn matches(files: &[(&str, &str)], request: &SearchRequest) -> Matches {
    match answered(files, request) {
        Ok(Searched::Matches(matches)) => matches,
        other => panic!("matches: {other:?}"),
    }
}

fn shown_lines(matches: &Matches) -> Vec<(String, u32, String)> {
    let mut lines = Vec::new();
    for group in &matches.groups {
        for block in &group.blocks {
            for line in &block.lines {
                lines.push((group.path.clone(), line.number, line.text.clone()));
            }
        }
    }
    lines
}

const CODE_AND_DOC: [(&str, &str); 3] = [
    ("README.md", "the marker_dir is set\n"),
    (
        "pkg/mark.py",
        "MARKER = 1\n\n\ndef marker_dir():\n    return \"marker_dir\"\n",
    ),
    ("config.json", "{\n  \"marker_dir\": true\n}\n"),
];

#[test]
fn a_pattern_that_does_not_parse_names_the_fault_its_column_and_the_literal_pattern() {
    let refused = answered(&CODE_AND_DOC, &request(Some("foo(")));
    let Err(SearchError::Pattern(error)) = refused else {
        panic!("a pattern refusal: {refused:?}");
    };
    assert_eq!(error.escaped, "foo\\(");
    let PatternFault::Syntax { reason, column } = error.fault else {
        panic!("a syntax fault");
    };
    assert_eq!((reason.as_str(), column), ("unclosed group", Some(4)));
}

#[test]
fn a_request_with_neither_pattern_nor_glob_is_refused() {
    let refused = answered(&CODE_AND_DOC, &request(None));
    assert!(
        matches!(refused, Err(SearchError::NothingAsked)),
        "{refused:?}"
    );
}

#[test]
fn source_files_come_first_and_each_match_keeps_its_number_and_its_enclosing_symbol() {
    let found = matches(&CODE_AND_DOC, &request(Some("marker_dir")));
    assert_eq!((found.matches, found.files, found.shown), (4, 3, 4));
    let paths: Vec<&str> = found
        .groups
        .iter()
        .map(|group| group.path.as_str())
        .collect();
    assert_eq!(paths, ["pkg/mark.py", "README.md", "config.json"]);
    let code = &found.groups[0].blocks;
    assert_eq!(code.len(), 1);
    let owner = code[0].owner.as_ref().unwrap_or_else(|| panic!("an owner"));
    assert_eq!(owner.stable_id, "py:pkg/mark.py::marker_dir");
    assert_eq!((owner.start_line, owner.end_line), (4, 5));
    let numbers: Vec<u32> = code[0].lines.iter().map(|line| line.number).collect();
    assert_eq!(numbers, [4, 5]);
    assert!(found.groups[1].blocks[0].owner.is_none());
}

#[test]
fn context_lines_merge_in_line_order_and_only_match_lines_count_toward_the_limit() {
    let files = [("a.py", "one\nhit\ntwo\nhit\nthree\nfour\nfive\n")];
    let mut asked = request(Some("hit"));
    asked.context = IntArg::from_usize(1);
    let found = matches(&files, &asked);
    let lines = &found.groups[0].blocks[0].lines;
    let shown: Vec<(u32, bool)> = lines
        .iter()
        .map(|line| (line.number, line.matched))
        .collect();
    assert_eq!(
        shown,
        [(1, false), (2, true), (3, false), (4, true), (5, false)]
    );
    assert_eq!(found.shown, 2);
}

#[test]
fn the_limit_cuts_in_answer_order_and_counts_what_it_left_out_per_file() {
    let files = [
        ("a.py", "hit = 1\nhit = 2\n"),
        ("b.py", "hit = 3\n"),
        ("c.py", "hit = 4\n"),
    ];
    let mut asked = request(Some("hit"));
    asked.limit = IntArg::from_usize(1);
    let found = matches(&files, &asked);
    assert_eq!((found.matches, found.shown), (4, 1));
    let rest: Vec<(&str, usize)> = (found.rest.iter())
        .map(|file| (file.path.as_str(), file.count))
        .collect();
    assert_eq!(rest, [("a.py", 1), ("b.py", 1), ("c.py", 1)]);
}

#[test]
fn a_text_in_three_places_prints_once_with_every_place_and_one_in_two_places_stays() {
    let files = [
        ("a.py", "import os\nx = 1\n"),
        ("b.py", "import os\nx = 2\n"),
        ("c.py", "import os\n"),
    ];
    let found = matches(&files, &request(Some("import os|x = ")));
    assert_eq!(found.repeats.len(), 1);
    assert_eq!(found.repeats[0].text, "import os");
    let places: Vec<Place> = ["a.py", "b.py", "c.py"]
        .map(|path| Place {
            path: path.to_owned(),
            line: 1,
        })
        .into();
    assert_eq!(found.repeats[0].places, places);
    let left = shown_lines(&found);
    assert_eq!(
        left,
        [
            ("a.py".to_owned(), 2, "x = 1".to_owned()),
            ("b.py".to_owned(), 2, "x = 2".to_owned())
        ]
    );
    assert_eq!(found.shown, 5);
}

#[test]
fn lock_vendored_and_minified_files_are_counts_and_a_generated_file_is_shown() {
    let long = format!("var hit={};\n", "x".repeat(2_500));
    let files = [
        ("Cargo.lock", "hit\nhit\n"),
        ("vendor/lib.py", "hit = 1\n"),
        ("static/app.min.js", "var hit=1;\n"),
        ("static/bundle.js", long.as_str()),
        ("rpc/flipt.pb.go", "package rpc // hit\n"),
    ];
    let found = matches(&files, &request(Some("hit")));
    let shown: Vec<&str> = found
        .groups
        .iter()
        .map(|group| group.path.as_str())
        .collect();
    assert_eq!(shown, ["rpc/flipt.pb.go"]);
    let noise: Vec<(NoiseClass, usize, Vec<&str>)> = (found.noise.iter())
        .map(|class| {
            let paths = class.files.iter().map(|file| file.path.as_str()).collect();
            (class.class, class.matches, paths)
        })
        .collect();
    assert_eq!(
        noise,
        [
            (NoiseClass::Lock, 2, vec!["Cargo.lock"]),
            (NoiseClass::Vendored, 1, vec!["vendor/lib.py"]),
            (
                NoiseClass::Minified,
                2,
                vec!["static/app.min.js", "static/bundle.js"]
            ),
        ]
    );
    assert_eq!((found.matches, found.files), (6, 5));
}

#[test]
fn a_path_that_names_a_noise_file_shows_it_and_only_a_minified_line_is_cut() {
    let long = format!("var hit={};\n", "x".repeat(2_500));
    let files = [("static/bundle.js", long.as_str())];
    let mut asked = request(Some("hit"));
    asked.path = Some("static/bundle.js".to_owned());
    let found = matches(&files, &asked);
    let line = &found.groups[0].blocks[0].lines[0];
    assert_eq!(line.text.chars().count(), super::noise::MINIFIED_KEEP_CHARS);
    assert_eq!(line.cut, Some(long.trim_end().chars().count() - 200));
}

#[test]
fn a_long_code_line_in_a_file_of_short_lines_is_never_cut() {
    let short = "a = 1\n".repeat(20);
    let text = format!("{short}hit = \"{}\"\n", "y".repeat(3_000));
    let found = matches(&[("a.py", text.as_str())], &request(Some("hit")));
    let line = &found.groups[0].blocks[0].lines[0];
    assert_eq!(line.cut, None);
    assert_eq!(line.text.len(), 3_008);
}

#[test]
fn a_listing_gives_each_file_its_lines_and_its_symbols_and_braces_expand() {
    let files = [
        (
            "src/a.py",
            "def one():\n    return 1\n\n\ndef two():\n    return 2\n",
        ),
        ("docs/b.md", "# title\ntext\n"),
        ("other/c.py", "x = 1\n"),
    ];
    let mut asked = request(None);
    asked.files = Some("{src,docs}/*".to_owned());
    let Ok(Searched::Listing(listing)) = answered(&files, &asked) else {
        panic!("a listing");
    };
    assert_eq!(listing.files, 2);
    let rows: Vec<(&str, &Measure)> = (listing.rows.iter())
        .map(|row| (row.path.as_str(), &row.measure))
        .collect();
    let counted = |lines, symbols| Measure::Counted { lines, symbols };
    assert_eq!(
        rows,
        [
            ("docs/b.md", &counted(2, None)),
            ("src/a.py", &counted(6, Some(2)))
        ]
    );
}

#[test]
fn a_folder_path_scopes_the_search_and_an_unknown_path_is_refused() {
    let files = [("a/x.py", "hit = 1\n"), ("b/y.py", "hit = 2\n")];
    let mut asked = request(Some("hit"));
    asked.path = Some("a".to_owned());
    let found = matches(&files, &asked);
    assert_eq!((found.files, found.searched), (1, 1));
    asked.path = Some("nowhere".to_owned());
    let refused = answered(&files, &asked);
    let Err(SearchError::UnknownPath { path }) = refused else {
        panic!("an unknown path: {refused:?}");
    };
    assert_eq!(path, "nowhere");
    asked.path = Some("../outside".to_owned());
    assert!(matches!(
        answered(&files, &asked),
        Err(SearchError::Refused(_))
    ));
}

#[test]
fn a_limit_or_a_context_out_of_range_gets_the_shared_bound_message() {
    let mut asked = request(Some("hit"));
    asked.context = IntArg::from_usize(11);
    let Err(SearchError::Refused(error)) = answered(&CODE_AND_DOC, &asked) else {
        panic!("a bound refusal");
    };
    assert_eq!(
        error.to_string(),
        "context takes a value from 0 through 10, got 11"
    );
    asked.context = IntArg::from_usize(0);
    asked.limit = IntArg::from_usize(1_001);
    let Err(SearchError::Refused(error)) = answered(&CODE_AND_DOC, &asked) else {
        panic!("a bound refusal");
    };
    assert_eq!(
        error.to_string(),
        "limit takes a value from 1 through 1000, got 1001"
    );
}

#[test]
fn ignore_case_matches_in_any_case_and_no_match_counts_the_files_searched() {
    let mut asked = request(Some("MARKER_DIR"));
    assert_eq!(matches(&CODE_AND_DOC, &asked).files, 0);
    assert_eq!(matches(&CODE_AND_DOC, &asked).searched, 3);
    asked.ignore_case = true;
    assert_eq!(matches(&CODE_AND_DOC, &asked).files, 3);
}

#[test]
fn the_literal_scan_names_the_first_line_of_each_file_up_to_the_cap() {
    let files = [
        ("b.md", "zz.no(such\n"),
        ("a.py", "x = 1\ny = 'zz.no(such'\n"),
        ("c.txt", "zz.no(such\n"),
    ];
    let scanned = Scanned::new(&files);
    let found = literal_files(&scanned.services, &scanned.reading(), "zz.no(such", 2);
    let (places, total) = found.value.unwrap_or_else(|error| panic!("{error}"));
    let places: Vec<(&str, u32)> = (places.iter())
        .map(|place| (place.path.as_str(), place.line))
        .collect();
    assert_eq!((places, total), (vec![("a.py", 2), ("b.md", 1)], 3));
}
