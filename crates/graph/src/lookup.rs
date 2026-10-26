//! What "the symbol the caller meant" is: a stable id, a `path::name`, a `path:line`, a file, a
//! dotted name or a bare name, resolved to definitions, and the sites that spell a definition from
//! outside it.

use std::borrow::Cow;
use std::ptr;

use facts::{StableId, split_ordinal};
use syntax::Language;

use crate::names::{Definition, NameIndex, Site};
use crate::owners::enclosing_symbol;
use crate::paths::stem;
use crate::scan::{FileFacts, in_name_graph};

const FORM_VALUES: [&str; 6] = [
    "stable_id",
    "path_name",
    "path_line",
    "path",
    "qualified_name",
    "name",
];

/// How the caller wrote a lookup target. Each form keeps its own scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetForm {
    StableId,
    PathName,
    PathLine,
    Path,
    QualifiedName,
    Name,
}

impl TargetForm {
    /// Return the wire value, such as `path_name`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        FORM_VALUES[self as usize]
    }
}

/// A lookup target after the parse: a stable id, a line of a file, or a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target<'t> {
    /// A stable id, or a `path::name` written as one.
    Id(StableId, TargetForm),
    /// A file and a line in it.
    Line(&'t str, u32),
    /// A bare or qualified name, with `::` between non-empty parts read as `.`.
    Named(Cow<'t, str>),
}

impl Target<'_> {
    /// Return the last name the target spells, such as `total` for `Invoice.total`, or `None` for
    /// a line.
    #[must_use]
    pub fn last_name(&self) -> Option<&str> {
        let qualified = match self {
            Target::Id(id, _) => id.qualname(),
            Target::Line(..) => return None,
            Target::Named(text) => text,
        };
        Some(last_name(split_ordinal(qualified).0))
    }
}

/// The definitions a lookup target found, whether the target named what it found, and its form.
#[derive(Clone, Debug)]
pub struct Lookup<'a> {
    pub definitions: Vec<Definition<'a>>,
    pub scoped: bool,
    pub form: TargetForm,
}

/// Resolve a target to definitions; a stable id or a `path::name` that no longer names its file
/// falls back to every definition of its last name, unscoped.
#[must_use]
pub fn resolve_definitions<'a>(index: &NameIndex<'a>, target: &str) -> Lookup<'a> {
    lookup_target(index, &parse_target(target))
}

/// Parse a target: a stable id, then a `path::name`, then a `path:line`, else a name.
#[must_use]
pub fn parse_target(text: &str) -> Target<'_> {
    if let Ok(id) = text.parse::<StableId>()
        && !id.path().starts_with(':')
    {
        return Target::Id(id, TargetForm::StableId);
    }
    if let Some(id) = path_name_id(text) {
        return Target::Id(id, TargetForm::PathName);
    }
    match path_line(text) {
        Some((path, line)) => Target::Line(path, line),
        None => Target::Named(qualified(text)),
    }
}

/// Return the stable id that a `path::qualname` text spells, with the prefix of the path's
/// language, or `None` for any other text.
#[must_use]
pub fn path_name_id(text: &str) -> Option<StableId> {
    let (path, qualname) = text.split_once("::")?;
    if path.contains(':') {
        return None;
    }
    let prefix = Language::from_path(path)?.extraction().prefix;
    format!("{prefix}:{path}::{qualname}").parse().ok()
}

/// Resolve a parsed target to definitions, each form at its own scope.
#[must_use]
pub fn lookup_target<'a>(index: &NameIndex<'a>, target: &Target<'_>) -> Lookup<'a> {
    match target {
        Target::Id(id, form) => Lookup {
            form: *form,
            ..by_stable_id(index, id)
        },
        Target::Line(path, line) => by_line(index, path, *line),
        Target::Named(text) if holds_file(index, text) => Lookup {
            definitions: Vec::new(),
            scoped: false,
            form: TargetForm::Path,
        },
        Target::Named(text) => by_name(index, text),
    }
}

/// Return whether the scan that `index` was built from read a file at `path`.
#[must_use]
pub fn holds_file(index: &NameIndex<'_>, path: &str) -> bool {
    index.files().iter().any(|file| file.path == path)
}

/// Return the definitions [`resolve_definitions`] finds for `target`.
#[must_use]
pub fn definitions_for<'a>(index: &NameIndex<'a>, target: &str) -> Vec<Definition<'a>> {
    resolve_definitions(index, target).definitions
}

/// Return the sites that spell a definition's name outside its own span, in scan order.
#[must_use]
pub fn references_to<'a>(index: &NameIndex<'a>, definition: &Definition<'a>) -> Vec<Site<'a>> {
    let symbol = definition.symbol();
    let inside = |site: &Site<'a>| {
        let line = site.reference.line;
        site.path() == definition.path() && symbol.line <= line && line <= symbol.end_line
    };
    index
        .sites(&symbol.name)
        .iter()
        .filter(|site| !inside(site))
        .copied()
        .collect()
}

fn by_name<'a>(index: &NameIndex<'a>, target: &str) -> Lookup<'a> {
    let (base, _) = split_ordinal(target);
    let candidates = index.definitions(last_name(base));
    let confirmed: Vec<Definition<'a>> = candidates
        .iter()
        .filter(|definition| path_confirms(definition, base))
        .copied()
        .collect();
    let dotted = base.contains('.');
    let form = if dotted {
        TargetForm::QualifiedName
    } else {
        TargetForm::Name
    };
    if !confirmed.is_empty() {
        return Lookup {
            definitions: confirmed,
            scoped: true,
            form,
        };
    }
    // A bare name promises no file, so it cannot have missed one; a dotted name can.
    Lookup {
        definitions: candidates.to_vec(),
        scoped: !dotted,
        form,
    }
}

// The innermost symbol whose span holds `line` in the name-graph file at `path`.
fn by_line<'a>(index: &NameIndex<'a>, path: &str, line: u32) -> Lookup<'a> {
    let files = index.files();
    let at = (files.iter()).position(|file| file.path == path && in_name_graph(file));
    let held = at.and_then(|at| innermost(files, at, line));
    Lookup {
        scoped: held.is_some(),
        definitions: held.into_iter().collect(),
        form: TargetForm::PathLine,
    }
}

fn innermost(files: &[FileFacts], at: usize, line: u32) -> Option<Definition<'_>> {
    let symbols = &files[at].facts.symbols;
    let symbol = enclosing_symbol(symbols, line)?;
    let position = symbols.iter().position(|each| ptr::eq(each, symbol))?;
    Some(Definition::new(files, at, position))
}

// `path:line`, where the path has a language suffix and the line is ASCII digits.
fn path_line(text: &str) -> Option<(&str, u32)> {
    let (path, digits) = text.rsplit_once(':')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let line = digits.parse().ok()?;
    let located = !path.contains(':') && Language::from_path(path).is_some();
    located.then_some((path, line))
}

// Qualified names join their parts with `.`, so `Type::method` reads as `Type.method`.
fn qualified(text: &str) -> Cow<'_, str> {
    if text.contains("::") && text.split("::").all(|part| !part.is_empty()) {
        Cow::Owned(text.replace("::", "."))
    } else {
        Cow::Borrowed(text)
    }
}

fn by_stable_id<'a>(index: &NameIndex<'a>, parsed: &StableId) -> Lookup<'a> {
    let (base, _) = split_ordinal(parsed.qualname());
    let all = index.definitions(last_name(base));
    let scoped: Vec<Definition<'a>> = all
        .iter()
        .filter(|definition| {
            definition.path() == parsed.path()
                && definition.symbol().id_qualname() == parsed.qualname()
        })
        .copied()
        .collect();
    let found = !scoped.is_empty();
    Lookup {
        definitions: if found { scoped } else { all.to_vec() },
        scoped: found,
        form: TargetForm::StableId,
    }
}

// The text after the last dot, or the whole text when that is empty.
fn last_name(base: &str) -> &str {
    base.rsplit_once('.')
        .map(|(_, last)| last)
        .filter(|last| !last.is_empty())
        .unwrap_or(base)
}

// Only a target longer than the qualified name says which file to look in.
fn path_confirms(definition: &Definition<'_>, target: &str) -> bool {
    let qualified = definition.symbol().id_qualname();
    let (base, _) = split_ordinal(&qualified);
    if target.chars().count() <= base.chars().count() {
        return false;
    }
    let dotted = format!("{}.{base}", stem(definition.path()).replace('/', "."));
    dotted == target || dotted.ends_with(&format!(".{target}"))
}
