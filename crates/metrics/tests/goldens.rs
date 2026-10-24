//! The Python complexity goldens: units, cc, SLOC, erosion, clone groups, and clone lines.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernel::Key;
use metrics::{
    DEFAULT_THRESHOLD, FileMetrics, MIN_INSTANCES, Unit, Unmeasurable, clone_lines, erosion,
    measure,
};
use serde::Deserialize;
use syntax::{Language, parse};

const TOLERANCE: f64 = 1e-9;

#[derive(Deserialize)]
struct Golden {
    status: String,
    error: Option<String>,
    #[serde(default)]
    units: Vec<GoldenUnit>,
    erosion: Option<f64>,
    clones: Option<GoldenClones>,
}

#[derive(Deserialize)]
struct GoldenUnit {
    name: String,
    start_line: u32,
    cc: u32,
    sloc: u32,
}

#[derive(Deserialize)]
struct GoldenClones {
    groups: Vec<Vec<[u32; 2]>>,
    sloc_lines: Vec<u32>,
}

#[test]
fn every_fixture_has_a_golden_and_every_golden_a_fixture() {
    assert_eq!(stems(&fixtures(), "py"), stems(&goldens(), "json"));
}

#[test]
fn the_measured_goldens_match() {
    let mut checked = 0;
    for stem in stems(&goldens(), "json") {
        let golden = golden(&stem);
        if golden.status != "ok" {
            continue;
        }
        checked += 1;
        let measured = measured(&stem).unwrap_or_else(|error| panic!("{stem}: {error}"));
        assert_eq!(
            unit_rows(&measured.units),
            golden_rows(&golden.units),
            "{stem}"
        );
        let expected = golden
            .erosion
            .unwrap_or_else(|| panic!("{stem}: no erosion"));
        let found = erosion(&measured.units, DEFAULT_THRESHOLD);
        assert!(
            (found - expected).abs() < TOLERANCE,
            "{stem}: {found} != {expected}"
        );
        let clones = golden.clones.unwrap_or_else(|| panic!("{stem}: no clones"));
        assert_eq!(clone_groups(&measured), clones.groups, "{stem}");
        assert_eq!(clone_lines(&[&measured]), [clones.sloc_lines], "{stem}");
    }
    assert!(checked > 0);
}

#[test]
fn a_rejected_golden_is_a_typed_failure() {
    let mut checked = 0;
    for stem in stems(&goldens(), "json") {
        let golden = golden(&stem);
        if golden.status == "ok" {
            continue;
        }
        checked += 1;
        let class = golden
            .error
            .unwrap_or_else(|| panic!("{stem}: no error class"));
        match measured(&stem) {
            Err(Unmeasurable::Tokenizer(error)) => assert_eq!(error.kind.class_name(), class),
            other => panic!("{stem}: expected a {class} refusal, found {other:?}"),
        }
    }
    assert!(checked > 0);
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn goldens() -> PathBuf {
    repository().join("tests/characterization/goldens/complexity")
}

fn fixtures() -> PathBuf {
    repository().join("tests/characterization/fixtures/complexity_py")
}

fn stems(directory: &Path, extension: &str) -> Vec<String> {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
    let mut stems: Vec<String> = entries
        .map(|entry| entry.unwrap_or_else(|error| panic!("{error}")).path())
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect();
    stems.sort();
    stems
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn golden(stem: &str) -> Golden {
    let path = goldens().join(format!("{stem}.json"));
    serde_json::from_str(&read(&path)).unwrap_or_else(|error| panic!("{stem}: {error}"))
}

fn measured(stem: &str) -> Result<FileMetrics, Unmeasurable> {
    let source = read(&fixtures().join(format!("{stem}.py")));
    let parsed = parse(Language::Python, Arc::from(source.as_str()))
        .unwrap_or_else(|error| panic!("{stem}: {error}"));
    measure(&parsed)
}

fn unit_rows(units: &[Unit]) -> Vec<(String, u32, u32, u32)> {
    let mut rows: Vec<_> = units
        .iter()
        .map(|unit| {
            let name = unit.qualname.rsplit('.').next().unwrap_or_default();
            (name.to_owned(), unit.start_line, unit.cc, unit.sloc)
        })
        .collect();
    rows.sort();
    rows
}

fn golden_rows(units: &[GoldenUnit]) -> Vec<(String, u32, u32, u32)> {
    let mut rows: Vec<_> = units
        .iter()
        .map(|unit| (unit.name.clone(), unit.start_line, unit.cc, unit.sloc))
        .collect();
    rows.sort();
    rows
}

fn groups(measured: &FileMetrics) -> Vec<Vec<[u32; 2]>> {
    let mut by_key: BTreeMap<Key, Vec<[u32; 2]>> = BTreeMap::new();
    for block in &measured.blocks {
        by_key
            .entry(block.key)
            .or_default()
            .push([block.start_line, block.end_line]);
    }
    by_key
        .into_values()
        .filter(|instances| instances.len() >= MIN_INSTANCES)
        .collect()
}

fn clone_groups(measured: &FileMetrics) -> Vec<Vec<[u32; 2]>> {
    let mut found = groups(measured);
    for instances in &mut found {
        instances.sort_unstable();
    }
    found.sort();
    found
}
