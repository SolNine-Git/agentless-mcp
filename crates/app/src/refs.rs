//! Fan-in: the sites that spell a target, attributed to the symbols that hold them and grouped by
//! file and evidence tier, and the symbols that the target's callers also reference.

mod shared;
mod sites;

use std::collections::{BTreeMap, BTreeSet};

use facts::Role;
use graph::{Definition, NameIndex, Resolver, Scopes, Site, references_to, resolve_definitions};
use kernel::{Bounded, Deadline};

use crate::bounds::within;
use crate::detached::answered_over;
use crate::ops::limits::MAX_LIMIT;
use crate::scan::RepoFacts;
use crate::{Error, IntArg, Noted, Reading, Services, Skipped};

/// A refs request: the target, the most sites, and whether to rank the shared callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsRequest {
    /// A stable id, a dotted name or a bare name.
    pub target: String,
    /// The most sites the groups keep, and the most shared-caller rows.
    pub limit: IntArg,
    /// Whether the answer ranks the symbols that the target's callers also reference.
    pub shared_callers: bool,
}

/// The fan-in of one target, with the shared callers when the request asked for them.
#[derive(Clone, Debug, PartialEq)]
pub struct RefsResult {
    /// The target as the caller wrote it.
    pub target: String,
    /// The sites the limit kept, grouped by file and tier, and the counts before the cut.
    pub groups: RefListing,
    /// The ranked symbols that the target's callers also reference.
    pub shared: Bounded<SharedCaller>,
    /// Whether the request asked for the shared callers.
    pub shared_callers: bool,
    /// Whether the target named what it found. When false, the answer is about a name.
    pub target_resolved: bool,
    /// The files the scan passed over.
    pub skipped: Vec<Skipped>,
    /// How many files the scan read.
    pub scanned: Option<usize>,
}

/// The groups that the limit kept, and the sites and files before the cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefListing {
    /// One group per file and tier, in path order, then tier order.
    pub rows: Vec<RefGroup>,
    /// How many sites spell the target.
    pub total: usize,
    /// The most sites the groups keep.
    pub limit: usize,
    /// How many files spell the target.
    pub files: usize,
}

impl RefListing {
    /// Returns how many sites the groups keep.
    #[must_use]
    pub fn shown(&self) -> usize {
        self.rows.iter().map(|group| group.sites.len()).sum()
    }

    /// Returns how many sites the limit left out.
    #[must_use]
    pub fn omitted(&self) -> usize {
        self.total.saturating_sub(self.shown())
    }

    /// Returns how many files the limit left out whole.
    #[must_use]
    pub fn files_omitted(&self) -> usize {
        let kept: BTreeSet<&str> = self.rows.iter().map(|group| group.path.as_str()).collect();
        self.files.saturating_sub(kept.len())
    }
}

/// The sites of one file at one evidence tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefGroup {
    /// The path below the repository root.
    pub path: String,
    /// The sites, in the order the limit kept them.
    pub sites: Vec<RefSite>,
    /// The tier's wire value, such as `imported`.
    pub tier: &'static str,
    /// The tier's phrase, such as `resolved-via-import`.
    pub tier_label: &'static str,
    /// False for a binding tier in a language with no scope analysis, where a local that
    /// shadows the name cannot be told from a caller.
    pub scope_checked: bool,
}

/// One site, attributed to the symbol whose span holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefSite {
    /// The line of the site.
    pub line: u32,
    /// The qualified name of the holding symbol, or the module-level marker.
    pub enclosing: String,
    /// The stable id of the holding symbol, if any.
    pub stable_id: Option<String>,
}

/// A symbol that the target's callers also reference.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedCaller {
    /// The candidate's stable id.
    pub stable_id: String,
    /// The candidate's file.
    pub path: String,
    /// The candidate's first line.
    pub line: u32,
    /// How many of the target's callers reference the candidate.
    pub overlap: usize,
    /// How many files those callers sit in.
    pub shared_files: usize,
    /// The file count damped by each caller's fan-out and by the candidate name's spread.
    pub score: f64,
    /// The shared callers, by path and line.
    pub callers: Vec<CallerRef>,
    /// Whether a test path defines the candidate.
    pub in_tests: bool,
}

/// One caller shared between the target and a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallerRef {
    /// The caller's qualified name.
    pub qualname: String,
    /// The caller's file.
    pub path: String,
    /// The caller's first line.
    pub line: u32,
}

/// Lists the sites that reference the target of `request` in the repository that `reading`
/// names. The scan's warnings come with either outcome.
#[must_use]
pub fn refs(
    services: &Services,
    reading: &Reading<'_>,
    request: &RefsRequest,
) -> Noted<Result<RefsResult, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    let stoplist = &reading.repo.config.stoplist;
    let loaded = RepoFacts::load(services, reading);
    let deadline = reading.deadline;
    loaded.map(|facts| {
        facts.and_then(|facts| {
            answered_over(facts, |facts| {
                fan_in(facts, stoplist, request, limit, deadline)
            })
        })
    })
}

fn fan_in(
    facts: &RepoFacts,
    stoplist: &BTreeSet<String>,
    request: &RefsRequest,
    limit: usize,
    deadline: Option<&Deadline>,
) -> Result<RefsResult, Error> {
    let index = NameIndex::build(&facts.files);
    let scopes = Scopes::build(&facts.files);
    let resolver = Resolver::new(&index, &scopes);
    let lookup = resolve_definitions(&index, &request.target);
    let target_ids: BTreeSet<String> = (lookup.definitions.iter())
        .map(|definition| definition.stable_id().to_string())
        .collect();
    let spellings = spellings(&resolver, &lookup.definitions, &target_ids);
    let sites = dedupe(&spellings);
    let names = names_by_path(&spellings);
    let kept = round_robin(&sites, limit);
    let groups = RefListing {
        rows: sites::group_sites(&kept, &names, &resolver, &target_ids),
        total: sites.len(),
        limit,
        files: sites.iter().map(Site::path).collect::<BTreeSet<_>>().len(),
    };
    let ranked = if request.shared_callers {
        shared::shared_callers(&sites, &target_ids, &index, stoplist, deadline)?
    } else {
        Vec::new()
    };
    Ok(RefsResult {
        target: request.target.clone(),
        groups,
        shared: Bounded::cut(ranked, limit),
        shared_callers: request.shared_callers,
        target_resolved: lookup.scoped,
        skipped: facts.skipped.clone(),
        scanned: Some(facts.files.len()),
    })
}

// Every spelling of each definition outside its own span, then the alias spellings.
fn spellings<'a>(
    resolver: &Resolver<'a>,
    definitions: &[Definition<'a>],
    target_ids: &BTreeSet<String>,
) -> Vec<Site<'a>> {
    let spelled: BTreeSet<&str> = (definitions.iter())
        .map(|definition| definition.symbol().name.as_str())
        .collect();
    let direct =
        (definitions.iter()).flat_map(|definition| references_to(resolver.index(), definition));
    direct
        .chain(alias_sites(resolver, target_ids, &spelled))
        .collect()
}

// A file that imported the target under another name calls it by that name. Only the import
// chain can rename, so it is walked first and the resolver confirms.
fn alias_sites<'a>(
    resolver: &Resolver<'a>,
    target_ids: &BTreeSet<String>,
    spelled: &BTreeSet<&str>,
) -> Vec<Site<'a>> {
    let mut found = Vec::new();
    for (path, scope) in resolver.scopes().iter() {
        for local in scope.member_names() {
            if spelled.contains(local)
                || !lands_on(&resolver.through_imports(local, path), target_ids)
            {
                continue;
            }
            let confirmed = resolver.resolve(local, path);
            if confirmed.is_some_and(|resolution| lands_on(&resolution.candidates, target_ids)) {
                found.extend(alias_spellings(resolver.index(), local, path));
            }
        }
    }
    found
}

// A parameter or local that reuses the alias spelling is not the alias.
fn alias_spellings<'a>(index: &NameIndex<'a>, local: &str, path: &str) -> Vec<Site<'a>> {
    let sites = index.sites(local).iter().copied();
    let aliases = sites.filter(|site| {
        site.path() == path && matches!(site.reference.role, Role::Import | Role::Reference)
    });
    aliases.collect()
}

fn lands_on(candidates: &[Definition<'_>], target_ids: &BTreeSet<String>) -> bool {
    (candidates.iter()).any(|definition| target_ids.contains(&definition.stable_id().to_string()))
}

// One site per line, the last spelling of the line, ordered by file then line.
fn dedupe<'a>(spellings: &[Site<'a>]) -> Vec<Site<'a>> {
    let unique: BTreeMap<(&str, u32), Site<'a>> = (spellings.iter())
        .map(|site| ((site.path(), site.reference.line), *site))
        .collect();
    unique.into_values().collect()
}

// The names each file spells the target by.
fn names_by_path<'a>(spellings: &[Site<'a>]) -> BTreeMap<&'a str, BTreeSet<&'a str>> {
    let mut names: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for site in spellings {
        let name = site.reference.name.as_str();
        names.entry(site.path()).or_default().insert(name);
    }
    names
}

// The limit is spent breadth first: one site per referencing file at a time, so a cut never
// drops a whole file while another keeps a second site. Files keep their path order.
fn round_robin<'a>(sites: &[Site<'a>], limit: usize) -> Vec<Site<'a>> {
    let mut by_file: Vec<Vec<Site<'a>>> = Vec::new();
    for site in sites {
        match by_file.last_mut() {
            Some(file) if file[0].path() == site.path() => file.push(*site),
            _ => by_file.push(vec![*site]),
        }
    }
    let rounds = by_file.iter().map(Vec::len).max().unwrap_or(0);
    let mut kept = Vec::new();
    for round in 0..rounds {
        kept.extend(by_file.iter().filter_map(|file| file.get(round).copied()));
    }
    kept.truncate(limit);
    kept
}

#[cfg(test)]
mod tests;
