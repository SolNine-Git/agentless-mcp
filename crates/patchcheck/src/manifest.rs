//! The dependency manifests reduced to what the import check reads: distribution names, the
//! interpreter floor, and the warnings of a manifest whose shape is not the expected one.

use std::collections::BTreeSet;

use toml::{Table, Value};

/// The file name of the PEP 621 manifest at a repository root.
pub const PYPROJECT_NAME: &str = "pyproject.toml";
/// The glob of the requirements files at a repository root.
pub const REQUIREMENTS_GLOB: &str = "requirements*.txt";

const PROJECT: &str = "project";
const REQUIRES_PYTHON: &str = "requires-python";
const OPTIONAL: &str = "optional-dependencies";
const DEPENDENCIES: &str = "dependencies";
const DEPENDENCY_GROUPS: &str = "dependency-groups";

/// One manifest: the normalized names it declares, its shape warnings, whether it parsed, and
/// its raw `requires-python` specifier or empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestParse {
    pub packages: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub parsed: bool,
    pub requires_python: String,
}

/// What a repository declares it depends on, the manifests it was read from, and the warnings of
/// that read. No source means that no manifest was read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredDependencies {
    pub packages: BTreeSet<String>,
    pub sources: Vec<String>,
    pub warnings: Vec<String>,
    pub requires_python: String,
}

impl DeclaredDependencies {
    /// Return whether at least one manifest was read.
    #[must_use]
    pub fn known(&self) -> bool {
        !self.sources.is_empty()
    }
}

// The strings of one manifest value, and the warning of a value with another shape.
struct Strings<'t> {
    values: Vec<&'t str>,
    warnings: Vec<String>,
}

/// Return the normalized distribution names that `[project] dependencies`, each list of
/// `[project.optional-dependencies]` and each list of `[dependency-groups]` declare. A document
/// that does not parse declares nothing and says why.
#[must_use]
pub fn parse_pyproject_dependencies(text: &str) -> ManifestParse {
    let document = match text.parse::<Table>() {
        Ok(document) => document,
        Err(error) => return unparsed(text, &error),
    };
    let mut read = Strings {
        values: Vec::new(),
        warnings: Vec::new(),
    };
    let mut floor = String::new();
    if let Some(Value::Table(project)) = document.get(PROJECT) {
        read.add(string_list(project.get(DEPENDENCIES), DEPENDENCIES));
        read.add(string_lists(project.get(OPTIONAL), OPTIONAL));
        match project.get(REQUIRES_PYTHON) {
            Some(Value::String(stated)) => stated.clone_into(&mut floor),
            Some(_) => read
                .warnings
                .push(format!("{REQUIRES_PYTHON} is not a string; ignored")),
            None => {}
        }
    }
    read.add(string_lists(
        document.get(DEPENDENCY_GROUPS),
        DEPENDENCY_GROUPS,
    ));
    ManifestParse {
        packages: read
            .values
            .iter()
            .map(|item| requirement_name(item))
            .filter(|name| !name.is_empty())
            .collect(),
        warnings: read.warnings,
        parsed: true,
        requires_python: floor,
    }
}

impl<'t> Strings<'t> {
    fn add(&mut self, other: Strings<'t>) {
        self.values.extend(other.values);
        self.warnings.extend(other.warnings);
    }
}

// The one-line form of a parser refusal, ended by where it happened as tomllib's messages are.
fn unparsed(text: &str, error: &toml::de::Error) -> ManifestParse {
    let message = error.message().lines().collect::<Vec<_>>().join(" ");
    let place = error
        .span()
        .map(|span| position(text, span.start))
        .unwrap_or_default();
    ManifestParse {
        warnings: vec![format!(
            "{PYPROJECT_NAME} did not parse: TOMLDecodeError: {message}{place}"
        )],
        ..ManifestParse::default()
    }
}

// The 1-based line and code point column of a byte offset.
fn position(text: &str, offset: usize) -> String {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    format!(" (at line {line}, column {column})")
}

fn string_list<'t>(value: Option<&'t Value>, place: &str) -> Strings<'t> {
    let mut read = Strings {
        values: Vec::new(),
        warnings: Vec::new(),
    };
    match value {
        None => {}
        Some(Value::Array(items)) => {
            read.values = items.iter().filter_map(Value::as_str).collect();
            let others = items.len() - read.values.len();
            if others > 0 {
                read.warnings.push(format!(
                    "{place} holds {others} non-string entries; ignored"
                ));
            }
        }
        Some(_) => read
            .warnings
            .push(format!("{place} is not a list; ignored")),
    }
    read
}

fn string_lists<'t>(value: Option<&'t Value>, place: &str) -> Strings<'t> {
    let mut read = Strings {
        values: Vec::new(),
        warnings: Vec::new(),
    };
    match value {
        None => {}
        Some(Value::Table(lists)) => {
            let mut keys: Vec<&String> = lists.keys().collect();
            keys.sort_unstable();
            for key in keys {
                read.add(string_list(lists.get(key), &format!("{place}.{key}")));
            }
        }
        Some(_) => read
            .warnings
            .push(format!("{place} is not a table; ignored")),
    }
    read
}

/// Return the normalized distribution names that a requirements file declares. Option lines are
/// skipped, not followed.
#[must_use]
pub fn parse_requirements(text: &str) -> BTreeSet<String> {
    text::splitlines(text, false)
        .into_iter()
        .filter_map(|raw| {
            let line = text::strip(raw.split('#').next().unwrap_or_default());
            if line.is_empty() || line.starts_with('-') {
                return None;
            }
            let name = requirement_name(line);
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

// The distribution a requirement names: its constraints after `;`, `@`, `[` or a version
// specifier are not its identity.
pub(crate) fn requirement_name(specification: &str) -> String {
    let head = specification.split(';').next().unwrap_or_default();
    let head = text::strip(head.split('@').next().unwrap_or_default());
    if !head.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return String::new();
    }
    let end = head
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        .unwrap_or(head.len());
    normalize_distribution(&head[..end])
}

// PEP 503: each run of `-`, `_` and `.` becomes one `-`, then the name is lowercased.
pub(crate) fn normalize_distribution(name: &str) -> String {
    let mut joined = String::with_capacity(name.len());
    let mut in_run = false;
    for c in name.chars() {
        let separator = matches!(c, '-' | '_' | '.');
        if separator && !in_run {
            joined.push('-');
        } else if !separator {
            joined.push(c);
        }
        in_run = separator;
    }
    text::lower(&joined)
}

// The lowest interpreter that the first `>=` clause of a `requires-python` specifier admits. A
// digit is any Unicode decimal digit, and a number past u64 reads as u64::MAX.
pub(crate) fn python_floor(specifier: &str) -> Option<(u64, u64)> {
    specifier
        .match_indices(">=")
        .find_map(|(at, _)| floor_after(&specifier[at + 2..]))
}

fn floor_after(text: &str) -> Option<(u64, u64)> {
    let text = text.trim_start_matches(text::is_space);
    let (major, rest) = leading_number(text)?;
    let (minor, _) = leading_number(rest.strip_prefix('.')?)?;
    Some((major, minor))
}

fn leading_number(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|c: char| !text::is_decimal(c))
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let value = text[..end]
        .chars()
        .filter_map(text::decimal_value)
        .fold(0_u64, |value, digit| {
            value.saturating_mul(10).saturating_add(u64::from(digit))
        });
    Some((value, &text[end..]))
}
