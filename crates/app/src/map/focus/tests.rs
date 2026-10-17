use graph::NameIndex;
use text::testing::{bits, cases};

use super::{Known, focus_resolution, matches_stem, normalized_spellings, path_matches};
use crate::map::seed_weights;
use crate::map::tests::fixture_facts;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "read/focus";

// The repository, the entry, its files and the stable ids of the definitions it named.
type ResolutionCase = (String, String, Vec<String>, Vec<String>);
// The repository, the entries, each seeded file's weight bits, the unresolved entries and the ids.
type SeedingCase = (
    String,
    Vec<String>,
    Vec<(String, String)>,
    Vec<String>,
    Vec<String>,
);

fn spellings(entry: &str) -> Vec<String> {
    normalized_spellings(entry)
}

#[test]
fn each_entry_resolves_as_the_reference_resolves_it() {
    let all: Vec<ResolutionCase> = cases(MANIFEST_DIR, FIXTURE, "resolutions");
    assert!(!all.is_empty());
    for (repo, entry, paths, ids) in all {
        let facts = fixture_facts(&repo);
        let index = NameIndex::build(&facts.files);
        let known: Known<'_> = facts.files.iter().map(|file| file.path.as_str()).collect();
        let (found, matched) = focus_resolution(&entry, &known, &index);
        let named: Vec<String> = matched.iter().map(|d| d.stable_id().to_string()).collect();
        assert_eq!((found, named), (paths, ids), "{repo}: {entry:?}");
    }
}

#[test]
fn the_seed_weights_split_each_entry_as_the_reference_splits_it() {
    let all: Vec<SeedingCase> = cases(MANIFEST_DIR, FIXTURE, "seedings");
    assert!(!all.is_empty());
    for (repo, entries, weights, unresolved, ids) in all {
        let facts = fixture_facts(&repo);
        let index = NameIndex::build(&facts.files);
        let known: Known<'_> = facts.files.iter().map(|file| file.path.as_str()).collect();
        let seeding = seed_weights(&entries, &known, &index);
        let found: Vec<(String, u64)> = (seeding.weights.iter())
            .map(|(path, weight)| (path.clone(), weight.to_bits()))
            .collect();
        let expected: Vec<(String, u64)> = (weights.iter())
            .map(|(path, hex)| (path.clone(), bits(hex)))
            .collect();
        assert_eq!(found, expected, "{repo}");
        assert_eq!(seeding.unresolved, unresolved, "{repo}");
        let named: Vec<String> = (seeding.definitions.iter())
            .map(|definition| definition.stable_id().to_string())
            .collect();
        assert_eq!(named, ids, "{repo}");
    }
}

#[test]
fn the_spellings_inside_an_entry_are_the_reference_spellings() {
    let all: Vec<(String, Vec<String>)> = cases(MANIFEST_DIR, FIXTURE, "spellings");
    assert!(!all.is_empty());
    for (entry, expected) in all {
        assert_eq!(spellings(&entry), expected, "{entry:?}");
    }
}

#[test]
fn a_traceback_frame_yields_its_path_and_every_tail() {
    let frame = "  File \"/home/u/proj/src/pkg/mod.py\", line 12, in f";
    let expected = [
        "/home/u/proj/src/pkg/mod.py",
        "u/proj/src/pkg/mod.py",
        "proj/src/pkg/mod.py",
        "src/pkg/mod.py",
        "pkg/mod.py",
        "mod.py",
    ];
    assert_eq!(spellings(frame), expected);
}

#[test]
fn a_url_loses_its_scheme_query_fragment_and_line_range() {
    let blob = "https://github.com/o/r/blob/main/src/a.py#L10-L20";
    assert_eq!(spellings(blob)[0], "github.com/o/r/blob/main/src/a.py");
    assert_eq!(spellings(blob).last().map(String::as_str), Some("a.py"));
    let query = "https://x.org/a/b.py?x=1#frag";
    assert_eq!(spellings(query), ["x.org/a/b.py", "a/b.py", "b.py"]);
}

#[test]
fn only_the_location_that_ends_the_entry_comes_off() {
    assert_eq!(spellings("src/a.py:12-40"), ["src/a.py", "a.py"]);
    assert_eq!(spellings("src/a.py#L12-40"), ["src/a.py", "a.py"]);
    assert_eq!(spellings("src/a.py:1:2"), ["src/a.py:1", "a.py:1"]);
    assert_eq!(spellings("a:1\n"), ["a"]);
    assert_eq!(spellings("src/a.py:12-"), ["a.py:12-"]);
}

#[test]
fn backslashes_walk_as_separators_and_the_entry_itself_never_returns() {
    let windows = "C:\\\\work\\\\repo\\\\src\\\\a.py:7";
    let expected = [
        "C:/work/repo/src/a.py",
        "work/repo/src/a.py",
        "repo/src/a.py",
        "src/a.py",
        "a.py",
    ];
    assert_eq!(spellings(windows), expected);
    assert_eq!(spellings("/abs/./x//y.py"), ["x/y.py", "y.py"]);
    assert!(spellings("quote").is_empty());
}

#[test]
fn a_stem_drops_only_a_final_suffix_with_a_name_before_it() {
    assert!(matches_stem("app/envelope.py", "envelope"));
    assert!(matches_stem("app/.env", ".env"));
    assert!(!matches_stem("app/x.", "app/x"));
    assert!(matches_stem("a.b.c", "a.b"));
}

#[test]
fn an_exact_path_beats_a_suffix_and_a_suffix_beats_a_stem() {
    let known: Known<'_> = ["config.go", "a/config.go", "b/config.go", "b/config.py"].into();
    assert_eq!(path_matches("config.go", &known), ["config.go"]);
    assert_eq!(path_matches("b/config.go", &known), ["b/config.go"]);
    let suffixed: Known<'_> = ["a/config.go", "b/config.go", "b/config.py"].into();
    assert_eq!(
        path_matches("config.go", &suffixed),
        ["a/config.go", "b/config.go"]
    );
    assert_eq!(
        path_matches("b/config", &suffixed),
        ["b/config.go", "b/config.py"]
    );
}
