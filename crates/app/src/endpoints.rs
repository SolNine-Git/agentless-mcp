//! The resolution of a target the caller typed to the definitions it names at its own scope, best
//! match first, which explain and refs read, the binding and coverage statuses of their answers,
//! and the endpoint that path starts from.

use facts::is_data_language;
use graph::{
    Definition, NameIndex, SymbolGraph, Target, TargetForm, holds_file, lookup_target, parse_target,
};
use kernel::Bounded;
use prompts::Template;
use prompts::Text;
use prompts::messages::{
    PATH_DATA_KEY_NO_EDGES, PATH_ENDPOINT_NO_MATCH, SPAN_SYMBOL_UNDEFINED, TARGET_CANDIDATES,
    TARGET_CANDIDATES_CUT, TARGET_IS_FILE, TARGET_NO_FILE, TARGET_NO_LINE, TARGET_SPELLED,
};
use syntax::Language;

use crate::span::data_key_span;
use crate::{Noted, Reading, Skipped};

// A path endpoint that names nothing lists at most this many definitions of its name.
const ENDPOINT_CANDIDATES: usize = 10;
const BINDING_VALUES: [&str; 3] = ["bound", "ambiguous", "not_found"];
const COVERAGE_VALUES: [&str; 3] = ["complete", "partial", "not_checked"];

/// Whether a target bound to definitions at its own scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingStatus {
    Bound,
    Ambiguous,
    NotFound,
}

impl BindingStatus {
    /// Return the wire value, such as `not_found`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        BINDING_VALUES[self as usize]
    }
}

/// What a target bound to: the status, the form the caller wrote, how many definitions, and, when
/// it bound to none, why, the definitions of its name, and the line that an answer gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetBinding {
    /// Whether the target bound to one definition, to several, or to none.
    pub status: BindingStatus,
    /// The wire value of the form, such as `path_name`.
    pub form: &'static str,
    /// How many definitions the target bound to.
    pub definitions: usize,
    /// Why a target that names a file bound to nothing, or empty.
    pub reason: String,
    /// The stable ids of the definitions of the target's name, when the target bound to none.
    pub candidates: Bounded<String>,
    /// The line an answer gives when the target bound to nothing, or empty.
    pub message: String,
}

/// Whether an answer read every file that a reference can sit in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverageStatus {
    Complete,
    Partial,
    NotChecked,
}

impl CoverageStatus {
    /// Return the wire value, such as `not_checked`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        COVERAGE_VALUES[self as usize]
    }
}

/// How much of the repository an answer checked for references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// Whether the check read every code file, some of them, or none.
    pub status: CoverageStatus,
    /// How many code files the scan skipped.
    pub skipped_code_files: usize,
}

impl Coverage {
    // A data file holds no reference, so a skipped data file leaves the check complete. A target
    // that bound to nothing was not looked for.
    pub(crate) fn of(binding: &TargetBinding, skipped: &[Skipped]) -> Coverage {
        let code =
            |entry: &&Skipped| !Language::from_path(&entry.path).is_some_and(is_data_language);
        let skipped_code_files = skipped.iter().filter(code).count();
        let status = match (binding.status, skipped_code_files) {
            (BindingStatus::NotFound, _) => CoverageStatus::NotChecked,
            (_, 0) => CoverageStatus::Complete,
            _ => CoverageStatus::Partial,
        };
        Coverage {
            status,
            skipped_code_files,
        }
    }

    pub(crate) fn not_checked() -> Coverage {
        Coverage {
            status: CoverageStatus::NotChecked,
            skipped_code_files: 0,
        }
    }
}

// The definitions a target names at its own scope, best first, the text that ranks them, the
// form, and the binding.
pub(crate) struct Bound<'a> {
    pub(crate) definitions: Vec<Definition<'a>>,
    pub(crate) key: String,
    pub(crate) form: TargetForm,
    pub(crate) binding: TargetBinding,
}

// A target never falls back outside its own scope. A qualified name that no file path confirms
// still names the definitions whose qualified name it spells, such as `Class.method`.
pub(crate) fn bind<'a>(
    index: &NameIndex<'a>,
    text: &str,
    limit: usize,
    first: &Template<Text>,
) -> Bound<'a> {
    let target = parse_target(text);
    let lookup = lookup_target(index, &target);
    let key = match &target {
        Target::Named(named) => named.to_string(),
        _ => text.to_owned(),
    };
    let mut ranked = rank_candidates(lookup.definitions, &key);
    let exact = exact_count(&ranked, &key);
    let named = matches!(target, Target::Named(_)) && !lookup.scoped && exact > 0;
    if named {
        ranked.truncate(exact);
    }
    if !(lookup.scoped || named) || ranked.is_empty() {
        let unbound = Unbound {
            index,
            target: &target,
            form: lookup.form,
        };
        return unbound.bound(text, &ranked, limit, first);
    }
    let status = if ranked.len() == 1 {
        BindingStatus::Bound
    } else {
        BindingStatus::Ambiguous
    };
    Bound {
        binding: TargetBinding {
            status,
            form: lookup.form.as_str(),
            definitions: ranked.len(),
            reason: String::new(),
            candidates: Bounded::cut(Vec::new(), 0),
            message: String::new(),
        },
        definitions: ranked,
        key,
        form: lookup.form,
    }
}

// The binding of a data file's id: the key it names, or the reason that expand gives.
pub(crate) fn data_key_binding(text: &str, missed: Option<&str>) -> TargetBinding {
    let form = match parse_target(text) {
        Target::Id(_, form) => form,
        _ => TargetForm::StableId,
    };
    let (status, definitions) = match missed {
        Some(_) => (BindingStatus::NotFound, 0),
        None => (BindingStatus::Bound, 1),
    };
    TargetBinding {
        status,
        form: form.as_str(),
        definitions,
        reason: missed.unwrap_or_default().to_owned(),
        candidates: Bounded::cut(Vec::new(), 0),
        message: missed.unwrap_or_default().to_owned(),
    }
}

// A target that bound to nothing, and the scan it was looked up in.
struct Unbound<'i, 'a, 't> {
    index: &'i NameIndex<'a>,
    target: &'i Target<'t>,
    form: TargetForm,
}

impl<'a> Unbound<'_, 'a, '_> {
    // The definitions of the name become the candidates, and none of them is bound.
    fn bound(
        &self,
        text: &str,
        candidates: &[Definition<'a>],
        limit: usize,
        first: &Template<Text>,
    ) -> Bound<'a> {
        let ids = candidates
            .iter()
            .map(|definition| definition.stable_id().to_string());
        let candidates = Bounded::cut(ids.collect(), limit);
        let reason = self.reason();
        let message = joined(first.render(text), &reason, &self.advice(&candidates));
        Bound {
            definitions: Vec::new(),
            key: text.to_owned(),
            form: self.form,
            binding: TargetBinding {
                status: BindingStatus::NotFound,
                form: self.form.as_str(),
                definitions: 0,
                reason,
                candidates,
                message,
            },
        }
    }

    // Only a target that names a file has a reason beyond the absence itself.
    fn reason(&self) -> String {
        let index = self.index;
        match self.target {
            Target::Id(id, _) if !holds_file(index, id.path()) => TARGET_NO_FILE.render(id.path()),
            Target::Id(id, _) => SPAN_SYMBOL_UNDEFINED.render((id.path(), id.qualname())),
            Target::Line(path, _) if !holds_file(index, path) => TARGET_NO_FILE.render(path),
            Target::Line(path, line) => TARGET_NO_LINE.render((path, *line as usize)),
            Target::Named(text) if self.form == TargetForm::Path => TARGET_IS_FILE.render(text),
            Target::Named(_) => String::new(),
        }
    }

    // The definitions of the name, or else the files that spell a name nothing declares.
    fn advice(&self, candidates: &Bounded<String>) -> String {
        let name = match self.target.last_name() {
            Some(name) if self.form != TargetForm::Path => name,
            _ => return String::new(),
        };
        let listed = candidates.rows().join(", ");
        match (candidates.total(), candidates.omitted()) {
            (0, _) => spelled(self.index, name),
            (total, 0) => TARGET_CANDIDATES.render((name, total, &listed)),
            (total, _) => {
                TARGET_CANDIDATES_CUT.render((name, candidates.rows().len(), total, &listed))
            }
        }
    }
}

fn spelled(index: &NameIndex<'_>, name: &str) -> String {
    match index.spelling_spread(name) as usize {
        0 => String::new(),
        files => TARGET_SPELLED.render((name, files)),
    }
}

fn joined(first: String, reason: &str, advice: &str) -> String {
    let mut line = first;
    if !reason.is_empty() {
        line.push_str(": ");
        line.push_str(reason);
    }
    if !advice.is_empty() {
        line.push_str(". ");
        line.push_str(advice);
    }
    line
}

// A definition whose qualified name is the text the caller typed outranks one that only ends
// with it; inside a band, the path and the line decide.
fn rank_candidates<'a>(definitions: Vec<Definition<'a>>, target: &str) -> Vec<Definition<'a>> {
    let mut ranked = definitions;
    ranked.sort_by_key(|definition| {
        let inexact = definition.symbol().qualname() != target;
        (inexact, definition.path(), definition.symbol().line)
    });
    ranked
}

// The exact qualified-name matches of `ranked`, or all of it when none is exact.
pub(crate) fn best_band<'r, 'a>(
    ranked: &'r [Definition<'a>],
    target: &str,
) -> &'r [Definition<'a>] {
    let exact = exact_count(ranked, target);
    if exact == 0 { ranked } else { &ranked[..exact] }
}

// How many definitions at the head of `ranked` have `target` as their qualified name.
fn exact_count(ranked: &[Definition<'_>], target: &str) -> usize {
    ranked
        .iter()
        .take_while(|definition| definition.symbol().qualname() == target)
        .count()
}

// One endpoint the caller named: the node it resolved to and its label, or why it resolved to
// none.
pub(crate) struct Located {
    pub(crate) node: String,
    pub(crate) label: String,
    pub(crate) message: String,
}

// A stable id and a path are exact. A name resolves only when its best band holds one symbol:
// picking one of several would put a guess at the end of a path that reads as evidence.
pub(crate) fn endpoint(index: &NameIndex<'_>, graph: &SymbolGraph<'_>, text: &str) -> Located {
    if let Some(definition) = graph.definitions().get(text) {
        return located(text.to_owned(), definition.symbol().qualname());
    }
    if graph.has_file(text) {
        return located(text.to_owned(), text.to_owned());
    }
    let bound = bind(index, text, ENDPOINT_CANDIDATES, &PATH_ENDPOINT_NO_MATCH);
    if bound.binding.status == BindingStatus::NotFound {
        return refused(text, bound.binding.message);
    }
    match best_band(&bound.definitions, &bound.key) {
        [only] => located(only.stable_id().to_string(), only.symbol().qualname()),
        best => {
            let ids: Vec<String> = best
                .iter()
                .map(|entry| entry.stable_id().to_string())
                .collect();
            refused(text, format!("{text} is ambiguous: {}", ids.join(", ")))
        }
    }
}

// A data key stays out of the graph, so a data file's id is no endpoint: the key has no edges, or
// the file does not hold it and the reason is the one expand gives. `None` for any other text.
pub(crate) fn data_key_endpoint(reading: &Reading<'_>, text: &str) -> Option<Noted<Located>> {
    let found = data_key_span(reading, text)?;
    Some(found.map(|span| {
        let message = match span {
            Ok(span) => PATH_DATA_KEY_NO_EDGES.render((text, span.symbol.name.as_str())),
            Err(missed) => missed.reason(),
        };
        refused(text, message)
    }))
}

fn located(node: String, label: String) -> Located {
    Located {
        node,
        label,
        message: String::new(),
    }
}

fn refused(text: &str, message: String) -> Located {
    Located {
        node: String::new(),
        label: text.to_owned(),
        message,
    }
}

#[cfg(test)]
pub(crate) mod scanned;
#[cfg(test)]
mod tests;
