use facts::{Facts, Ref, Role};
use graph::{FileFacts, Site};
use syntax::Language;

use super::{dedupe, names_by_path, round_robin};

fn file(path: &str, lines: &[u32]) -> FileFacts {
    let refs = lines.iter().map(|&line| Ref {
        name: format!("n{line}"),
        line,
        role: Role::Reference,
        qualifier: String::new(),
    });
    FileFacts {
        path: path.to_owned(),
        language: Language::Python,
        line_count: 100,
        facts: Facts {
            refs: refs.collect(),
            ..Facts::default()
        },
    }
}

fn sites(files: &[FileFacts]) -> Vec<Site<'_>> {
    let each = files.iter().flat_map(|file| {
        let refs = file.facts.refs.iter();
        refs.map(move |reference| Site { file, reference })
    });
    each.collect()
}

fn places(sites: &[Site<'_>]) -> Vec<(String, u32)> {
    let places = sites
        .iter()
        .map(|site| (site.path().to_owned(), site.reference.line));
    places.collect()
}

#[test]
fn one_site_per_line_in_file_then_line_order() {
    let files = [file("b.py", &[9, 3, 3]), file("a.py", &[5])];
    let spelled = sites(&files);
    let unique = dedupe(&spelled);
    let expected =
        [("a.py", 5), ("b.py", 3), ("b.py", 9)].map(|(path, line)| (path.to_owned(), line));
    assert_eq!(places(&unique), expected);
    let names = names_by_path(&spelled);
    assert_eq!(names["b.py"].len(), 2);
}

#[test]
fn the_limit_takes_one_site_per_file_in_turn() {
    let files = [
        file("a.py", &[1, 2, 3]),
        file("b.py", &[7]),
        file("c.py", &[4, 8]),
    ];
    let unique = dedupe(&sites(&files));
    let kept = round_robin(&unique, 5);
    let expected = [
        ("a.py", 1),
        ("b.py", 7),
        ("c.py", 4),
        ("a.py", 2),
        ("c.py", 8),
    ];
    let expected = expected.map(|(path, line)| (path.to_owned(), line));
    assert_eq!(places(&kept), expected);
    assert_eq!(round_robin(&unique, 100).len(), 6);
}
