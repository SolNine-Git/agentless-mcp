//! Fan-in: the sites that spell a target, attributed to the symbols that hold them and grouped by
//! file and evidence tier, and the symbols that the target's callers also reference.

mod shared;
mod sites;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use facts::{Declared, Role};
use graph::{
    Aliases, Definition, FileFacts, NameIndex, Resolver, Scopes, Site, TargetForm, references_to,
};
use kernel::{Bounded, Deadline};

use crate::bounds::within;
use crate::detached::answered_over;
use crate::endpoints::{
    BindingStatus, Bound, Coverage, Scope, TARGET_WORDING, TargetBinding, bind, data_key_binding,
};
use crate::ops::limits::MAX_LIMIT;
use crate::scan::RepoFacts;
use crate::span::{Missed, SymbolSpan, data_key_span};
use crate::symbol_graph::{CallGraph, CallScan};
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
    /// Whether the target named what it found.
    pub target_resolved: bool,
    /// The stable id of the one definition a name or a qualified name resolved to.
    pub definition: Option<String>,
    /// How many definitions the target resolved to.
    pub definitions: usize,
    /// What the target bound to, and why it bound to nothing.
    pub binding: TargetBinding,
    /// How much of the repository the answer checked for references.
    pub coverage: Coverage,
    /// The key's name when the target is a data key, which has no fan-in.
    pub data_key: Option<String>,
    /// Why a data file's id names no key, or `None`.
    pub unresolved: Option<String>,
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
    /// How many of those sites sit at a binding tier, same-file or resolved-via-import.
    pub bound: usize,
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
    with_limit(reading, request, |limit| {
        let loaded = RepoFacts::load(services, reading);
        loaded.map(|facts| {
            facts.and_then(|facts| {
                answered_over(facts, |facts| scanned(facts, reading, request, limit))
            })
        })
    })
}

// The fan-in over a resolver of the scan's own.
fn scanned(
    facts: &RepoFacts,
    reading: &Reading<'_>,
    request: &RefsRequest,
    limit: usize,
) -> Result<RefsResult, Error> {
    let files: Vec<&FileFacts> = facts.files.iter().collect();
    let index = NameIndex::build(&files);
    let scopes = Scopes::build(&files);
    let resolver = Resolver::new(&index, &scopes);
    let fan = FanIn {
        resolver: &resolver,
        aliases: &Aliases::build(&resolver),
        skipped: &facts.skipped,
        root: &reading.repo.root,
        stoplist: &reading.repo.config.stoplist,
        deadline: reading.deadline,
    };
    fan_in(&fan, request, limit)
}

// Lists the sites as `refs` does, over the graph that one trace call reads for all its cards. The
// warnings of the call's scan come first, as the scan of `refs` gives them.
pub(crate) fn refs_in(
    reading: &Reading<'_>,
    request: &RefsRequest,
    graph: &CallGraph<'_, '_>,
    scan: &CallScan,
) -> Noted<Result<RefsResult, Error>> {
    with_limit(reading, request, |limit| {
        let (skipped, root) = (scan.skipped.clone(), reading.repo.root.clone());
        let stoplist = reading.repo.config.stoplist.clone();
        let request = request.clone();
        let answered = graph.over(move |graph| {
            let fan = FanIn {
                resolver: graph.resolver(),
                aliases: graph.aliases(),
                skipped: &skipped,
                root: &root,
                stoplist: &stoplist,
                deadline: graph.deadline(),
            };
            fan_in(&fan, &request, limit)
        });
        let mut diagnostics = scan.warnings.clone();
        diagnostics.extend(answered.diagnostics);
        Noted {
            value: answered.value,
            diagnostics,
        }
    })
}

// A refused limit and a data key need no scan; any other request gets its limit to `fanned`.
fn with_limit(
    reading: &Reading<'_>,
    request: &RefsRequest,
    fanned: impl FnOnce(usize) -> Noted<Result<RefsResult, Error>>,
) -> Noted<Result<RefsResult, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    if let Some(found) = data_key_span(reading, &request.target) {
        return found.map(|span| Ok(keyed(request, limit, span)));
    }
    fanned(limit)
}

// What one fan-in reads: the resolver and the aliases of the scan, the files it passed over, the
// repository's root and stoplist, and the deadline.
struct FanIn<'f, 'a> {
    resolver: &'f Resolver<'f, 'a>,
    aliases: &'f Aliases<'a>,
    skipped: &'f [Skipped],
    root: &'f Path,
    stoplist: &'f BTreeSet<String>,
    deadline: Option<&'f Deadline>,
}

fn fan_in(fan: &FanIn<'_, '_>, request: &RefsRequest, limit: usize) -> Result<RefsResult, Error> {
    let (resolver, index) = (fan.resolver, fan.resolver.index());
    let scope = Scope {
        index,
        skipped: fan.skipped,
        root: fan.root,
    };
    let bound = bind(&scope, &request.target, limit, &TARGET_WORDING);
    let scanned = RefsResult {
        coverage: Coverage::of(&bound.binding, fan.skipped),
        skipped: fan.skipped.to_vec(),
        scanned: Some(index.files().len()),
        ..nothing_listed(request, limit, bound.binding.clone())
    };
    if bound.binding.status == BindingStatus::NotFound {
        return Ok(scanned);
    }
    let target_ids: BTreeSet<String> = (bound.definitions.iter())
        .map(|definition| definition.stable_id().to_string())
        .collect();
    let spellings = spellings(fan, &bound.definitions, &target_ids);
    let sites = dedupe(&spellings);
    let names = names_by_path(&spellings);
    let tiers = sites::site_tiers(&sites, &names, resolver, &target_ids);
    let kept = round_robin(&sites, limit);
    let groups = RefListing {
        rows: sites::group_sites(&kept, &tiers),
        total: sites.len(),
        bound: sites::bound_count(&sites, &tiers),
        limit,
        files: sites.iter().map(Site::path).collect::<BTreeSet<_>>().len(),
    };
    let ranked = if request.shared_callers {
        shared::shared_callers(&sites, &target_ids, index, fan.stoplist, fan.deadline)?
    } else {
        Vec::new()
    };
    Ok(RefsResult {
        groups,
        shared: Bounded::cut(ranked, limit),
        definition: single_definition(&bound),
        definitions: bound.definitions.len(),
        ..scanned
    })
}

// A data key stays out of the name graph, so nothing references it. A key that the file does
// not hold gets the reason that expand gives.
fn keyed(request: &RefsRequest, limit: usize, span: Result<SymbolSpan, Missed>) -> RefsResult {
    match span {
        Ok(span) => RefsResult {
            data_key: Some(span.symbol.name),
            ..nothing_listed(request, limit, data_key_binding(&request.target, None))
        },
        Err(missed) => {
            let reason = missed.reason();
            let binding = data_key_binding(&request.target, Some(&reason));
            RefsResult {
                unresolved: Some(reason),
                ..nothing_listed(request, limit, binding)
            }
        }
    }
}

fn nothing_listed(request: &RefsRequest, limit: usize, binding: TargetBinding) -> RefsResult {
    RefsResult {
        target: request.target.clone(),
        groups: RefListing {
            rows: Vec::new(),
            total: 0,
            bound: 0,
            limit,
            files: 0,
        },
        shared: Bounded::cut(Vec::new(), limit),
        shared_callers: request.shared_callers,
        target_resolved: binding.status != BindingStatus::NotFound,
        definition: None,
        definitions: 0,
        binding,
        coverage: Coverage::not_checked(),
        data_key: None,
        unresolved: None,
        skipped: Vec::new(),
        scanned: None,
    }
}

// The one definition that a target other than a stable id bound to.
fn single_definition(bound: &Bound<'_>) -> Option<String> {
    match bound.definitions.as_slice() {
        [only] if bound.form != TargetForm::StableId => Some(only.stable_id().to_string()),
        _ => None,
    }
}

// Each definition's spellings outside its span in its family, then the alias spellings, less the
// paths through a module outside the repository and the lines that only declare (a row is a line).
fn spellings<'a>(
    fan: &FanIn<'_, 'a>,
    definitions: &[Definition<'a>],
    target_ids: &BTreeSet<String>,
) -> Vec<Site<'a>> {
    let resolver = fan.resolver;
    let spelled: BTreeSet<&str> = (definitions.iter())
        .map(|definition| definition.symbol().name.as_str())
        .collect();
    let direct = (definitions.iter()).flat_map(|definition| {
        let family = definition.file().language.family();
        let sites = references_to(resolver.index(), definition).into_iter();
        sites.filter(move |site| site.file.language.family() == family)
    });
    let mut declared: HashMap<&'a str, Declared<'a>> = HashMap::new();
    let all = direct.chain(alias_sites(fan, target_ids, &spelled));
    all.filter(|site| {
        !resolver.names_outside(site.reference, site.path()) && !only_declares(&mut declared, site)
    })
    .collect()
}

// Each file's declared names are collected once, on its first site.
fn only_declares<'a>(declared: &mut HashMap<&'a str, Declared<'a>>, site: &Site<'a>) -> bool {
    let facts = &site.file.facts;
    let file =
        (declared.entry(site.path())).or_insert_with(|| Declared::of(&facts.symbols, &facts.refs));
    file.only_declares(&site.reference.name, site.reference.line)
}

// A file that imported the target under another name calls it by that name. Only the import
// chain can rename, so the chains that reach the target come first and the resolver confirms.
fn alias_sites<'a>(
    fan: &FanIn<'_, 'a>,
    target_ids: &BTreeSet<String>,
    spelled: &BTreeSet<&str>,
) -> Vec<Site<'a>> {
    let resolver = fan.resolver;
    let mut found = Vec::new();
    for (path, local) in fan.aliases.reaching(target_ids) {
        if spelled.contains(local) {
            continue;
        }
        let confirmed = resolver.resolve(local, path);
        if confirmed.is_some_and(|resolution| lands_on(&resolution.candidates, target_ids)) {
            found.extend(alias_spellings(resolver.index(), local, path));
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
