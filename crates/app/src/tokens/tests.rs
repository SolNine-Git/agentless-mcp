use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use prompts::tool_descriptions::{ORIENT, REPO_MAP};
use tiktoken_rs::CoreBPE;

use super::{Chars4, Cl100k, TokenCounter, loaded_once};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
// The band of a real BPE count over the chars/4 estimate on a map, which the tool descriptions
// publish. It is looser than the measured spread, so only a change in kind of the density fails.
const MAP_RATIO_FLOOR: f64 = 1.10;
const MAP_RATIO_CEILING: f64 = 1.35;

#[test]
fn chars4_counts_code_points_and_rounds_down() {
    assert_eq!(Chars4.count(""), 0);
    assert_eq!(Chars4.count("abc"), 0);
    assert_eq!(Chars4.count("abcd"), 1);
    // Four code points in sixteen bytes are one token, not four.
    assert_eq!(Chars4.count("\u{1f600}\u{1f600}\u{1f600}\u{1f600}"), 1);
    assert_eq!(Chars4.count(&"x".repeat(4001)), 1000);
}

fn goldens_root() -> PathBuf {
    [MANIFEST_DIR, "../../tests/characterization/goldens"]
        .iter()
        .collect()
}

fn golden(path: &str) -> String {
    let path = goldens_root().join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn cl100k_counts_each_golden_and_its_prefixes_as_the_reference_does() {
    let counter = Cl100k::new().unwrap_or_else(|error| panic!("{error}"));
    let cases: Vec<(String, Option<usize>, usize)> =
        io::testing::cases(MANIFEST_DIR, "read/tokens", "goldens");
    assert!(cases.len() > 200);
    for (path, cut, expected) in cases {
        let text = golden(&path);
        let shown: String = match cut {
            Some(cut) => text.chars().take(cut).collect(),
            None => text,
        };
        assert_eq!(counter.count(&shown), expected, "{path} cut {cut:?}");
    }
}

#[test]
fn cl100k_counts_each_edge_text_as_the_reference_does() {
    let counter = Cl100k::new().unwrap_or_else(|error| panic!("{error}"));
    let cases: Vec<(String, usize)> = io::testing::cases(MANIFEST_DIR, "read/tokens", "texts");
    assert!(!cases.is_empty());
    for (text, expected) in cases {
        assert_eq!(counter.count(&text), expected, "{text:?}");
    }
}

fn recorded_counts() -> BTreeMap<String, usize> {
    let cases: Vec<(String, Option<usize>, usize)> =
        io::testing::cases(MANIFEST_DIR, "read/tokens", "goldens");
    cases
        .into_iter()
        .filter(|(_, cut, _)| cut.is_none())
        .map(|(path, _, count)| (path, count))
        .collect()
}

fn goldens_ending(dir: &Path, suffix: &str) -> Vec<String> {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    let mut found = Vec::new();
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
            .path();
        if path.is_dir() {
            found.extend(goldens_ending(&path, suffix));
        } else if path.to_string_lossy().ends_with(suffix) {
            let below = path
                .strip_prefix(goldens_root())
                .expect("a golden is below the root");
            found.push(below.to_string_lossy().into_owned());
        }
    }
    found.sort();
    found
}

fn as_f64(count: usize) -> f64 {
    f64::from(u32::try_from(count).expect("a golden holds fewer than 2^32 tokens"))
}

fn ratio(path: &str, counts: &BTreeMap<String, usize>) -> Option<f64> {
    let real = counts.get(path);
    let real = real.unwrap_or_else(|| panic!("{path}: read/tokens.json records no count"));
    let estimated = Chars4.count(&golden(path));
    (estimated > 0).then(|| as_f64(*real) / as_f64(estimated))
}

#[test]
fn the_map_goldens_stay_inside_the_published_ratio() {
    let counts = recorded_counts();
    let maps = goldens_ending(&goldens_root().join("map"), ".map.txt");
    assert!(!maps.is_empty(), "no map goldens");
    for path in maps {
        let ratio = ratio(&path, &counts).unwrap_or_else(|| panic!("{path}: the estimate is 0"));
        assert!(
            (MAP_RATIO_FLOOR..=MAP_RATIO_CEILING).contains(&ratio),
            "{path}: real/estimated is {ratio:.3}, outside the band \
             {MAP_RATIO_FLOOR}-{MAP_RATIO_CEILING}. Measure the figures in the agent guide and \
             in the repo_map and orient tool descriptions again, then move this band."
        );
    }
}

#[test]
fn the_published_band_is_the_band_under_test() {
    let low = ((MAP_RATIO_FLOOR - 1.0) * 100.0).round();
    let high = ((MAP_RATIO_CEILING - 1.0) * 100.0).round();
    let published = format!("a real BPE count runs {low}-{high}% higher");
    for (tool, description) in [("orient", ORIENT), ("repo_map", REPO_MAP)] {
        assert!(
            description.contains(&published),
            "the {tool} description does not publish {published:?}, the band that this file \
             tests. Change both or neither."
        );
    }
}

#[test]
fn the_estimator_is_not_uniformly_low() {
    let counts = recorded_counts();
    let texts = goldens_ending(&goldens_root(), ".txt");
    let ratios: Vec<f64> = texts
        .iter()
        .filter_map(|path| ratio(path, &counts))
        .collect();
    assert!(!ratios.is_empty(), "no .txt golden renders any text");
    let low = ratios.iter().copied().fold(f64::INFINITY, f64::min);
    let high = ratios.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        low < 1.0,
        "every .txt golden now estimates low: the spread in the agent guide and its claim of \
         both directions are stale (min {low:.3})"
    );
    assert!(
        high > 1.15,
        "no .txt golden estimates materially low any more: the figures for id-dense views are \
         stale (max {high:.3})"
    );
}

#[test]
fn every_cl100k_counter_reads_the_one_loaded_encoding() {
    let first = Cl100k::new().unwrap_or_else(|error| panic!("{error}"));
    let second = Cl100k::new().unwrap_or_else(|error| panic!("{error}"));
    assert!(std::ptr::eq(first.encoding, second.encoding));
}

#[test]
fn a_failed_load_is_returned_and_the_next_call_loads_again() {
    let lock = OnceLock::new();

    let failed = loaded_once(&lock, || Err::<CoreBPE, _>("no tables"));
    let message = failed.err().map(|error| error.to_string());
    assert_eq!(
        message.as_deref(),
        Some("--token-counter tiktoken could not load cl100k_base: no tables")
    );
    assert!(lock.get().is_none());

    let loaded = loaded_once(&lock, tiktoken_rs::cl100k_base);
    let encoding = loaded.unwrap_or_else(|error| panic!("{error}"));
    assert!(std::ptr::eq(
        encoding,
        lock.get().expect("the load filled the lock")
    ));
}
