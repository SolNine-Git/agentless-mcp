use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use text::testing::{cases, document};

use crate::stream::StreamKinds;
use crate::tests::hex;
use crate::{Language, NodeId, parse, tokens};

#[derive(Deserialize)]
struct Source {
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamCase {
    name: String,
    language: String,
    stream: String,
    omit: u32,
    without: String,
}

// A language, its comment kinds that the grammar knows, and its indent-block kinds.
type TableRow = (String, Vec<String>, Vec<String>);

#[test]
fn every_language_has_the_reference_comment_and_indent_block_kinds() {
    let rows: Vec<TableRow> = cases(env!("CARGO_MANIFEST_DIR"), "stream", "tables");
    let names: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(names, Language::ALL.map(Language::name));
    for (name, comments, indent_blocks) in &rows {
        let language = Language::from_name(name).expect("a known language");
        let table = language.table();
        let measured = table
            .measured
            .map_or(&[][..], |measured| measured.clones.indent_blocks);
        assert_eq!(table.comments, comments.as_slice(), "{name} comments");
        assert_eq!(measured, indent_blocks.as_slice(), "{name} indent blocks");
        let kinds = StreamKinds::of(language);
        assert_eq!(*kinds.comments, language.kinds(table.comments), "{name}");
        let blocks = kinds
            .indent_blocks
            .map_or_else(|| language.kinds(&[]), Clone::clone);
        assert_eq!(blocks, language.kinds(measured), "{name}");
    }
}

#[test]
fn every_stream_equals_the_reference_stream() {
    let sources: BTreeMap<String, String> =
        document::<Vec<Source>>(env!("CARGO_MANIFEST_DIR"), "parse")
            .into_iter()
            .map(|source| (source.name, source.source))
            .collect();
    let found: Vec<StreamCase> = cases(env!("CARGO_MANIFEST_DIR"), "stream", "cases");
    assert_eq!(found.len(), sources.len());
    let mut languages = BTreeSet::new();
    for case in &found {
        let language = Language::from_name(&case.language).expect("a known language");
        let parsed = parse(language, Arc::from(sources[&case.name].as_str()))
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let root = parsed.root();
        assert_eq!(
            tokens(&parsed, root, None).join(" "),
            case.stream,
            "{}",
            case.name
        );
        assert!(case.omit < parsed.node_count(), "{}", case.name);
        let without = tokens(&parsed, root, Some(NodeId(case.omit))).join(" ");
        let digest = hex(&Sha256::digest(without.as_bytes()));
        assert_eq!(
            digest, case.without,
            "{} without node {}",
            case.name, case.omit
        );
        languages.insert(language);
    }
    assert_eq!(languages.len(), Language::ALL.len());
}
