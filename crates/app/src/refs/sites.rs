//! The fan-in groups: each site attributed to the symbol that holds it, and each line tiered on
//! its own bindings.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ptr;

use facts::{Ref, Role, StableId, Symbol};
use graph::{FileFacts, Resolution, Resolver, Site, Tier, enclosing_symbol};
use syntax::Language;

use super::{RefGroup, RefSite};
use crate::MODULE_LEVEL;

// The sites of one file at one tier, with the file.
type Grouped<'a> = BTreeMap<(&'a str, Tier), (&'a FileFacts, Vec<RefSite>)>;

// The tier of each line that spells the target, by file.
pub(super) type LineTiers<'a> = HashMap<&'a str, BTreeMap<u32, Tier>>;

// Every file of `sites` is tiered once, before the limit cuts the sites.
pub(super) fn site_tiers<'a>(
    sites: &[Site<'a>],
    names: &BTreeMap<&str, BTreeSet<&str>>,
    resolver: &Resolver<'a>,
    target_ids: &BTreeSet<String>,
) -> LineTiers<'a> {
    let mut tiers: LineTiers<'a> = HashMap::new();
    let no_names = BTreeSet::new();
    for site in sites {
        let path = site.path();
        let file_names = names.get(path).unwrap_or(&no_names);
        (tiers.entry(path))
            .or_insert_with(|| line_tiers(site.file, file_names, resolver, target_ids));
    }
    tiers
}

// How many of `sites` sit at a binding tier.
pub(super) fn bound_count(sites: &[Site<'_>], tiers: &LineTiers<'_>) -> usize {
    sites
        .iter()
        .filter(|site| tier_of(tiers, site).binds())
        .count()
}

pub(super) fn group_sites<'a>(sites: &[Site<'a>], tiers: &LineTiers<'a>) -> Vec<RefGroup> {
    let mut grouped: Grouped<'a> = BTreeMap::new();
    for site in sites {
        let symbol = enclosing_symbol(&site.file.facts.symbols, site.reference.line);
        let group = grouped
            .entry((site.path(), tier_of(tiers, site)))
            .or_insert((site.file, Vec::new()));
        group.1.push(site_row(site, symbol));
    }
    (grouped.into_iter())
        .map(|((_, tier), (file, rows))| file_group(file, tier, rows))
        .collect()
}

fn tier_of(tiers: &LineTiers<'_>, site: &Site<'_>) -> Tier {
    let lines = tiers.get(site.path());
    let tier = lines.and_then(|lines| lines.get(&site.reference.line));
    tier.copied().unwrap_or(Tier::Ambiguous)
}

fn site_row(site: &Site<'_>, symbol: Option<&Symbol>) -> RefSite {
    let line = site.reference.line;
    match symbol {
        Some(symbol) => RefSite {
            line,
            enclosing: symbol.qualname(),
            stable_id: Some(StableId::of(site.file.language, site.path(), symbol).to_string()),
        },
        None => RefSite {
            line,
            enclosing: MODULE_LEVEL.to_owned(),
            stable_id: None,
        },
    }
}

// A binding tier in a language with no scope analysis cannot tell a shadowing local from a
// caller, so the group says the locals were not checked.
fn file_group(file: &FileFacts, tier: Tier, sites: Vec<RefSite>) -> RefGroup {
    let binding = tier.binds();
    RefGroup {
        path: file.path.clone(),
        sites,
        tier: tier.as_str(),
        tier_label: tier.label(),
        scope_checked: !binding || scope_analysed(file.language),
    }
}

fn scope_analysed(language: Language) -> bool {
    language == Language::Python
}

// A row is a line, so it takes the best tier of the target's spellings on it: `helper(x).helper`
// calls the target although its last spelling is a member.
fn line_tiers(
    file: &FileFacts,
    names: &BTreeSet<&str>,
    resolver: &Resolver<'_>,
    target_ids: &BTreeSet<String>,
) -> BTreeMap<u32, Tier> {
    let mut bound = resolver.resolved_sites(file, Some(names)).peekable();
    let mut tiers: BTreeMap<u32, Tier> = BTreeMap::new();
    let spellings = file.facts.refs.iter();
    for reference in spellings.filter(|reference| names.contains(reference.name.as_str())) {
        let resolution = bound.next_if(|(site, _)| ptr::eq(*site, reference));
        let resolution = resolution.map(|(_, resolution)| resolution);
        let tier = site_tier(reference, &file.path, resolution, resolver, target_ids);
        let best = tiers.entry(reference.line).or_insert(Tier::Ambiguous);
        *best = (*best).min(tier);
    }
    tiers
}

// An import is tiered by the binding it creates, and a qualifier by the binding it spells. Any
// other site that binds nothing is a spelling only.
fn site_tier(
    reference: &Ref,
    path: &str,
    resolution: Option<Resolution<'_>>,
    resolver: &Resolver<'_>,
    target_ids: &BTreeSet<String>,
) -> Tier {
    let named = matches!(reference.role, Role::Import | Role::ModuleQualifier);
    let resolution = match resolution {
        None if named => resolver.resolve(&reference.name, path),
        resolution => resolution,
    };
    let Some(resolution) = resolution else {
        return Tier::Ambiguous;
    };
    let candidates = resolution.candidates.iter();
    let lands = candidates.map(|definition| definition.stable_id().to_string());
    if lands.into_iter().any(|id| target_ids.contains(&id)) {
        resolution.tier
    } else {
        Tier::Ambiguous
    }
}
