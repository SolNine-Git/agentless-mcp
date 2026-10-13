use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use syntax::Language;

use crate::names::Definition;
use crate::scan::FileFacts;
use crate::symbols::{AmbiguousReference, Endpoint, SymbolEdge};

pub(super) const CORPORA: [&str; 12] = [
    "ext_communities",
    "ext_python",
    "ext_scripts",
    "fixtures_tree",
    "repo_go",
    "repo_nested",
    "repo_py",
    "repo_py_tests",
    "repo_ts",
    "shapes",
    "tier1",
    "tier2",
];

pub(super) struct Corpus {
    pub(super) name: &'static str,
    sections: Map<String, Value>,
}

#[derive(Deserialize)]
struct FileCase {
    path: String,
    language: String,
    line_count: u32,
    facts: facts::Facts,
}

impl Corpus {
    pub(super) fn load(name: &'static str) -> Corpus {
        let file = format!("corpora/{name}");
        Corpus {
            name,
            sections: text::testing::document(env!("CARGO_MANIFEST_DIR"), &file),
        }
    }

    pub(super) fn all() -> impl Iterator<Item = Corpus> {
        CORPORA.into_iter().map(Corpus::load)
    }

    // A section the corpus leaves out reads as None.
    pub(super) fn section<T: DeserializeOwned>(&self, section: &str) -> Option<Vec<T>> {
        let value = self.sections.get(section)?.clone();
        let cases = serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("{}: {section}: {error}", self.name));
        Some(cases)
    }

    pub(super) fn cases<T: DeserializeOwned>(&self, section: &str) -> Vec<T> {
        self.section(section)
            .unwrap_or_else(|| panic!("{}: no section {section}", self.name))
    }

    pub(super) fn files(&self) -> Vec<FileFacts> {
        self.cases::<FileCase>("files")
            .into_iter()
            .map(|case| FileFacts {
                language: Language::from_name(&case.language)
                    .unwrap_or_else(|| panic!("{}: language {}", self.name, case.language)),
                path: case.path,
                line_count: case.line_count,
                facts: case.facts,
            })
            .collect()
    }
}

pub(super) fn bits(value: f64) -> String {
    format!("0x{:016x}", value.to_bits())
}

pub(super) fn id(definition: &Definition<'_>) -> String {
    definition.stable_id().to_string()
}

pub(super) fn ids(definitions: &[Definition<'_>]) -> Value {
    Value::from(definitions.iter().map(id).collect::<Vec<_>>())
}

pub(super) fn endpoint(endpoint: &Endpoint<'_>) -> Value {
    json!([
        endpoint.node(),
        endpoint.path(),
        endpoint.line(),
        endpoint.label(),
        endpoint.is_symbol()
    ])
}

pub(super) fn edge(edge: &SymbolEdge<'_>) -> Value {
    json!([
        endpoint(edge.source()),
        endpoint(edge.target()),
        edge.name(),
        edge.relation().as_str(),
        edge.tier().as_str()
    ])
}

pub(super) fn reference(reference: &AmbiguousReference<'_>) -> Value {
    json!([
        endpoint(reference.source()),
        reference.name(),
        reference.relation().as_str()
    ])
}
