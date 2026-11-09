use regex::Regex;

use super::{Gathered, shaped};
use crate::search::files::parsed_text;
use crate::search::lines::{FileHits, hits_in};

fn hits(path: &str, text: &str) -> FileHits {
    let regex = Regex::new("hit").unwrap_or_else(|error| panic!("{error}"));
    hits_in(path, text, &regex, 0).unwrap_or_else(|| panic!("a hit in {path}"))
}

fn kept_lines(files: &[FileHits]) -> Vec<(&str, usize, usize)> {
    (files.iter())
        .map(|file| (file.path.as_str(), file.matches, file.lines.len()))
        .collect()
}

#[test]
fn only_a_file_that_the_limit_can_show_keeps_its_lines_and_its_text_and_every_count_stays() {
    let files = [
        ("README.md", "hit\n"),
        ("z.py", "hit\n"),
        ("Cargo.lock", "hit\nhit\n"),
        ("a.py", "hit\nhit\nhit\n"),
    ];
    let mut gathered = Gathered::new(None, 2, 0);
    for (path, text) in files {
        gathered.add(hits(path, text), || parsed_text(path, text.as_bytes()));
    }
    assert_eq!(
        kept_lines(&gathered.plain),
        [("README.md", 1, 0), ("z.py", 1, 0), ("a.py", 3, 2)]
    );
    assert_eq!(kept_lines(&gathered.noisy), [("Cargo.lock", 2, 0)]);
    let texts: Vec<&str> = gathered.texts.keys().map(String::as_str).collect();
    assert_eq!(texts, ["a.py"]);
    let answer = shaped(gathered).matches;
    let rest: Vec<(&str, usize)> = (answer.rest.iter())
        .map(|file| (file.path.as_str(), file.count))
        .collect();
    assert_eq!((answer.matches, answer.files, answer.shown), (7, 4, 2));
    assert_eq!(rest, [("a.py", 1), ("z.py", 1), ("README.md", 1)]);
}
