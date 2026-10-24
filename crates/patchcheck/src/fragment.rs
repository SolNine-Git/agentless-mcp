//! One edit reduced to what the checks read: its two sides with elisions expanded, its anchor in
//! the pre-patch file, and the facts of the dedented replacement and search texts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use checks::Finding;
use facts::{Facts, Import, Ref, Symbol};
use patch::{Edit, Located};
use syntax::{Language, Unparsable};
use text::{posix_name, split_suffix};

use crate::lexer::{CallSite, call_sites};
use crate::repo::PatchBaseline;
use crate::{CHECK_COVERAGE, Parse, exception_class, gap};

const NO_EXTENSION: &str = "(no extension)";

pub(crate) struct Fragment<'e> {
    pub(crate) edit: &'e Edit,
    pub(crate) language: Language,
    // The dedented replacement, as text and as code points.
    pub(crate) replace_text: String,
    pub(crate) replace_chars: Vec<char>,
    // The replacement before the dedent, so a symbol's own indentation can be read.
    pub(crate) replace_columns: Vec<String>,
    // The 1-based lines the search text occupies in the pre-patch file; (0, 0) when it is
    // absent or occurs more than once.
    pub(crate) search_span: (u32, u32),
    pub(crate) introduced: Vec<Symbol>,
    pub(crate) replaced_names: BTreeSet<String>,
    pub(crate) replaced_qualnames: BTreeSet<String>,
    pub(crate) imports: Vec<Import>,
    pub(crate) refs: Vec<Ref>,
    pub(crate) calls: Vec<CallSite>,
}

impl Fragment<'_> {
    pub(crate) fn path(&self) -> &str {
        &self.edit.path
    }

    pub(crate) fn base_line(&self) -> u32 {
        self.search_span.0
    }

    // The file line of a line of the replacement block, or 0 for a block with no anchor.
    pub(crate) fn file_line(&self, fragment_line: u32) -> u32 {
        if self.base_line() == 0 || fragment_line == 0 {
            return 0;
        }
        self.base_line() + fragment_line - 1
    }

    // Whether the symbol's declaration began at column 1 of the block as written: the dedent
    // that made the block parse moves a method's `def` there too.
    pub(crate) fn is_module_level(&self, symbol: &Symbol) -> bool {
        let Some(index) = usize::try_from(symbol.line)
            .ok()
            .and_then(|line| line.checked_sub(1))
        else {
            return false;
        };
        self.replace_columns
            .get(index)
            .and_then(|line| line.chars().next())
            .is_some_and(|first| !text::is_space(first))
    }
}

// Each edit that a grammar reads becomes a fragment; an edit in another language, or one whose
// sides do not parse, becomes a coverage gap.
pub(crate) fn fragments<'e>(
    edits: &'e [Edit],
    facts: &PatchBaseline<'_>,
    parse: &Parse,
) -> (Vec<Fragment<'e>>, Vec<Finding>) {
    let mut fragments = Vec::new();
    let mut unsupported: BTreeMap<&str, usize> = BTreeMap::new();
    let mut notes = Vec::new();
    for edit in edits {
        let name = posix_name(&edit.path);
        let Some(language) = Language::from_path(name) else {
            *unsupported.entry(suffix_label(name)).or_default() += 1;
            continue;
        };
        match fragment(edit, language, facts, parse) {
            Ok(fragment) => fragments.push(fragment),
            Err(error) => notes.push(gap(
                CHECK_COVERAGE,
                &edit.path,
                "not checked: this edit's replacement block could not be parsed".to_owned(),
                format!("{}: {error}", exception_class(&error)),
            )),
        }
    }
    notes.extend(unsupported.into_iter().map(|(suffix, count)| {
        gap(
            CHECK_COVERAGE,
            "",
            format!(
                "not checked: {count} edit(s) to {suffix} files, \
                 which this build has no grammar for"
            ),
            format!("suffix {suffix}"),
        )
    }));
    (fragments, notes)
}

fn suffix_label(name: &str) -> &str {
    match split_suffix(name).1 {
        "" => NO_EXTENSION,
        found => found,
    }
}

fn fragment<'e>(
    edit: &'e Edit,
    language: Language,
    facts: &PatchBaseline<'_>,
    parse: &Parse,
) -> Result<Fragment<'e>, Unparsable> {
    let text = facts.text(&edit.path).unwrap_or_default();
    let (search, replace) = resolved(edit, text);
    let replace_text = text::dedent(&replace);
    let added = extract(parse, language, &replace_text)?;
    let replaced = extract(parse, language, &text::dedent(&search))?.symbols;
    let replace_chars: Vec<char> = replace_text.chars().collect();
    let calls = call_sites(&replace_chars, &added.refs, &added.symbols);
    Ok(Fragment {
        edit,
        language,
        replace_text,
        replace_chars,
        replace_columns: replace.split('\n').map(str::to_owned).collect(),
        search_span: span(text, &search),
        replaced_names: replaced.iter().map(|symbol| symbol.name.clone()).collect(),
        replaced_qualnames: replaced.iter().map(Symbol::qualname).collect(),
        introduced: added.symbols,
        imports: added.imports,
        refs: added.refs,
        calls,
    })
}

pub(crate) fn extract(parse: &Parse, language: Language, text: &str) -> Result<Facts, Unparsable> {
    parse(language, Arc::from(text)).map(|parsed| facts::extract(&parsed))
}

// Both sides with their `...` elisions expanded against the whole file, as the applier expands
// them; an edit whose elision cannot be expanded keeps its sides as written.
fn resolved(edit: &Edit, text: &str) -> (String, String) {
    match patch::resolve_elisions(&edit.search, &edit.replace, text) {
        Ok(elided) => (elided.search, elided.replace),
        Err(_) => (edit.search.clone(), edit.replace.clone()),
    }
}

fn span(text: &str, search: &str) -> (u32, u32) {
    match patch::locate(text, search) {
        Located::Unique {
            first_line,
            last_line,
        } => (line_number(first_line), line_number(last_line)),
        Located::Absent | Located::Ambiguous { .. } => (0, 0),
    }
}

pub(crate) fn line_number(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

// What the whole patch defines, and what it takes out of each file and does not put back: a
// change split over two blocks is one patch.
pub(crate) struct PatchNames<'f> {
    introduced: BTreeSet<&'f str>,
    removed_by_path: BTreeMap<&'f str, BTreeSet<&'f str>>,
    replaced_by_path: BTreeMap<&'f str, BTreeSet<&'f str>>,
}

impl<'f> PatchNames<'f> {
    pub(crate) fn of(fragments: &'f [Fragment<'_>]) -> PatchNames<'f> {
        let mut added: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut replaced: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for fragment in fragments {
            let names = fragment
                .introduced
                .iter()
                .map(|symbol| symbol.name.as_str());
            added.entry(fragment.path()).or_default().extend(names);
            let names = fragment.replaced_names.iter().map(String::as_str);
            replaced.entry(fragment.path()).or_default().extend(names);
        }
        let removed_by_path = replaced
            .iter()
            .map(|(&path, names)| {
                let kept = &added[path];
                (path, names.difference(kept).copied().collect())
            })
            .collect();
        PatchNames {
            introduced: added.into_values().flatten().collect(),
            removed_by_path,
            replaced_by_path: replaced,
        }
    }

    pub(crate) fn introduced(&self) -> &BTreeSet<&'f str> {
        &self.introduced
    }

    pub(crate) fn removed_in(&self, path: &str) -> Option<&BTreeSet<&'f str>> {
        self.removed_by_path.get(path)
    }

    pub(crate) fn replaced_in(&self, path: &str) -> Option<&BTreeSet<&'f str>> {
        self.replaced_by_path.get(path)
    }
}

// One gap for each edited file that no symbol table was supplied for.
pub(crate) fn unscanned_gaps(
    check: &'static str,
    fragments: &[Fragment<'_>],
    facts: &PatchBaseline<'_>,
) -> Vec<Finding> {
    let missing: BTreeSet<&str> = fragments
        .iter()
        .map(Fragment::path)
        .filter(|path| facts.file(path).is_none())
        .collect();
    missing
        .into_iter()
        .map(|path| {
            gap(
                check,
                path,
                "not checked: no symbol table was supplied for this file".to_owned(),
                "file absent from the scan".to_owned(),
            )
        })
        .collect()
}

// One gap for each edited language whose vocabulary `check` does not know.
pub(crate) fn language_gaps(
    check: &'static str,
    fragments: &[Fragment<'_>],
    what: &str,
) -> Vec<Finding> {
    let languages: BTreeSet<&str> = fragments
        .iter()
        .filter(|fragment| fragment.language != Language::Python)
        .map(|fragment| fragment.language.name())
        .collect();
    languages
        .into_iter()
        .map(|language| {
            gap(
                check,
                "",
                format!("not checked: no {what} is known for {language}"),
                format!("language {language}"),
            )
        })
        .collect()
}
