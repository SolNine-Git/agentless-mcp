//! The vendored grammar archive equals its manifest, file by file.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::PathBuf;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use syntax::Language;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    pack: String,
    version: String,
    asset: String,
    asset_sha256: String,
    archive: String,
    archive_sha256: String,
    language: Vec<Grammar>,
    file: Vec<File>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grammar {
    name: String,
    symbol: String,
    repo: String,
    rev: String,
    license: String,
    abi: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    path: String,
    size: usize,
    sha256: String,
    origin: String,
}

fn vendor() -> PathBuf {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    PathBuf::from(dir).join("vendor")
}

fn manifest() -> Manifest {
    let path = vendor().join("MANIFEST.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            write!(text, "{byte:02x}").expect("a String takes every write");
            text
        })
}

fn members(archive: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let decoder = ruzstd::decoding::StreamingDecoder::new(archive).expect("a zstd frame");
    let mut tar = tar::Archive::new(decoder);
    let mut found = BTreeMap::new();
    for entry in tar.entries().expect("a tar stream") {
        let mut entry = entry.expect("a tar entry");
        let path = entry.path().expect("a path").to_string_lossy().into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("the entry's data");
        assert!(
            found.insert(path.clone(), data).is_none(),
            "{path} appears twice"
        );
    }
    found
}

#[test]
fn the_archive_holds_exactly_the_files_the_manifest_lists() {
    let manifest = manifest();
    assert_eq!(manifest.version, syntax::PACK_VERSION);
    assert_eq!(
        manifest.archive,
        format!("grammars-{}.tar.zst", manifest.version)
    );
    let archive_path = vendor().join(&manifest.archive);
    let archive = std::fs::read(&archive_path)
        .unwrap_or_else(|error| panic!("{}: {error}", archive_path.display()));
    assert_eq!(
        sha256(&archive),
        manifest.archive_sha256,
        "the archive's sha256"
    );
    let members = members(&archive);
    let listed: Vec<&str> = manifest
        .file
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let held: Vec<&str> = members.keys().map(String::as_str).collect();
    assert_eq!(held, listed, "the archive's files, in path order");
    for file in &manifest.file {
        let data = &members[&file.path];
        assert_eq!(data.len(), file.size, "{}: size", file.path);
        assert_eq!(sha256(data), file.sha256, "{}: sha256", file.path);
        assert!(!file.origin.is_empty(), "{}: origin", file.path);
    }
}

#[test]
fn the_manifest_names_each_language_with_its_compiled_abi() {
    let manifest = manifest();
    assert!(manifest.pack.ends_with("tree-sitter-language-pack"));
    assert_eq!(
        manifest.asset,
        format!("parser-sources-{}.tar.zst", manifest.version)
    );
    assert_eq!(manifest.asset_sha256.len(), 64);
    let names: Vec<&str> = manifest
        .language
        .iter()
        .map(|grammar| grammar.name.as_str())
        .collect();
    let expected: Vec<&str> = Language::ALL
        .iter()
        .map(|language| language.name())
        .collect();
    assert_eq!(names, expected);
    for grammar in &manifest.language {
        let language = Language::from_name(&grammar.name).expect("a known language");
        assert_eq!(language.abi_version(), grammar.abi, "{}: ABI", grammar.name);
        assert!(!grammar.symbol.is_empty() && grammar.repo.starts_with("https://github.com/"));
        assert_eq!(grammar.rev.len(), 40, "{}: a full commit id", grammar.name);
        assert!(
            ["MIT", "Apache-2.0"].contains(&grammar.license.as_str()),
            "{}",
            grammar.name
        );
    }
}
