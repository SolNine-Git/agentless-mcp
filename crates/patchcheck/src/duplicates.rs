//! `near_duplicates`: an introduced function whose body normalizes to the token stream of an
//! existing one.

use std::collections::{BTreeMap, BTreeSet};

use checks::{Finding, Severity};
use facts::{Symbol, SymbolKind};
use sha2::{Digest, Sha256};
use syntax::{Language, Unparsable};

use crate::fragment::Fragment;
use crate::repo::RepoFacts;
use crate::{CHECK_NEAR_DUPLICATES, MAX_DUPLICATE_SITES, gap, more, quoted};

// Below this many normalized tokens, equal bodies are a coincidence, not a duplicate.
const MIN_BODY_TOKENS: usize = 12;

type Key = [u8; 32];

// One existing definition that a body key points back at.
pub(crate) struct Site<'a> {
    path: &'a str,
    line: u32,
    name: String,
}

// Every readable function body of the repository by its key, each list in (path, line) order.
pub(crate) type Bodies<'a> = BTreeMap<Key, Vec<Site<'a>>>;

pub(crate) fn check(
    fragments: &[Fragment<'_>],
    facts: &RepoFacts<'_>,
) -> Result<Vec<Finding>, Unparsable> {
    let mut findings: Vec<Finding> = unread_gap(facts).into_iter().collect();
    let index = facts.bodies()?;
    for fragment in fragments {
        let lines: Vec<&str> = fragment.replace_text.split('\n').collect();
        for symbol in fragment.introduced.iter().filter(|symbol| has_body(symbol)) {
            let Some(key) = body_key(&lines, symbol, fragment.language)? else {
                continue;
            };
            let sites: Vec<&Site<'_>> = index
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|site| !is_the_replaced_site(fragment, site))
                .collect();
            if !sites.is_empty() {
                findings.push(duplicate_finding(fragment, symbol, &sites));
            }
        }
    }
    Ok(findings)
}

fn unread_gap(facts: &RepoFacts<'_>) -> Option<Finding> {
    let unread: BTreeSet<&str> = facts
        .files()
        .map(|file| file.path.as_str())
        .filter(|path| facts.text(path).is_none())
        .collect();
    let first = unread.first()?;
    Some(gap(
        CHECK_NEAR_DUPLICATES,
        "",
        format!(
            "partially checked: {} of {} scanned files had no text supplied, \
             so their bodies were not compared",
            unread.len(),
            facts.file_count()
        ),
        format!("first unread: {first}"),
    ))
}

fn has_body(symbol: &Symbol) -> bool {
    matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
}

// The site the patch replaces, by its line inside the anchored search text or by its qualified
// name: `ThisClass.run` and `OtherClass.run` are two definitions.
fn is_the_replaced_site(fragment: &Fragment<'_>, site: &Site<'_>) -> bool {
    if site.path != fragment.path() {
        return false;
    }
    let (start, end) = fragment.search_span;
    if start > 0 && (start..=end).contains(&site.line) {
        return true;
    }
    fragment.replaced_qualnames.contains(&site.name)
}

fn duplicate_finding(fragment: &Fragment<'_>, symbol: &Symbol, sites: &[&Site<'_>]) -> Finding {
    let listed: Vec<String> = sites
        .iter()
        .take(MAX_DUPLICATE_SITES)
        .map(|site| format!("{}:{} ({})", site.path, site.line, site.name))
        .collect();
    Finding {
        check: CHECK_NEAR_DUPLICATES,
        severity: Severity::Advisory,
        message: format!(
            "the body the patch gives {} normalises to the same token stream \
             as {} existing definition(s); you may already have this",
            quoted(&symbol.name),
            sites.len()
        ),
        path: fragment.path().to_owned(),
        line: fragment.file_line(symbol.line),
        evidence: format!(
            "already at {}{}",
            listed.join(", "),
            more(sites.len(), MAX_DUPLICATE_SITES)
        ),
    }
}

pub(crate) fn existing_bodies<'a>(facts: &RepoFacts<'a>) -> Result<Bodies<'a>, Unparsable> {
    let mut index: Bodies<'a> = BTreeMap::new();
    for file in facts.files() {
        let Some(text) = facts.text(&file.path) else {
            continue;
        };
        let lines: Vec<&str> = text.split('\n').collect();
        for symbol in file.facts.symbols.iter().filter(|symbol| has_body(symbol)) {
            if let Some(key) = body_key(&lines, symbol, file.language)? {
                index.entry(key).or_default().push(Site {
                    path: &file.path,
                    line: symbol.line,
                    name: symbol.qualname(),
                });
            }
        }
    }
    for sites in index.values_mut() {
        sites.sort_by(|left, right| (left.path, left.line).cmp(&(right.path, right.line)));
    }
    Ok(index)
}

// The key of a body without its declaration line, dedented and normalized, or `None` for a body
// of fewer than MIN_BODY_TOKENS tokens.
fn body_key(
    lines: &[&str],
    symbol: &Symbol,
    language: Language,
) -> Result<Option<Key>, Unparsable> {
    let (Ok(start), Ok(end)) = (
        usize::try_from(symbol.line),
        usize::try_from(symbol.end_line),
    ) else {
        return Ok(None);
    };
    if end <= start || start >= lines.len() {
        return Ok(None);
    }
    let body = text::dedent(&lines[start..end.min(lines.len())].join("\n"));
    let stream = patch::normalized_stream(&body, Some(language))?;
    if text::split_whitespace(&stream).count() < MIN_BODY_TOKENS {
        return Ok(None);
    }
    Ok(Some(Sha256::digest(stream.as_bytes()).into()))
}
