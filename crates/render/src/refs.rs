//! The fan-in's two forms: the sites grouped by file and tier, or the shared callers, in the
//! text, and every group with the shared callers in the JSON.

use app::{
    ROW_INDENT, RefGroup, RefListing, RefSite, RefsResult, SharedCaller, file_site, locator,
};
use facts::split_ordinal;
use json::{fixed, round_half_even};
use prompts::messages::{
    DATA_KEY_NO_FAN_IN, REFS_ABSENT, REFS_ABSENT_COMPLETE, REFS_CALLERS_UNLISTED,
    REFS_CANDIDATES_UNLISTED, REFS_GROUP_HEADER, REFS_HEADER, REFS_HEADER_SHARED,
    REFS_SHARED_CANDIDATE, REFS_TARGET_UNRESOLVED, REFS_UNLISTED, REFS_UNLISTED_FILES,
};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json, skipped_line};
use crate::{Rendered, View};

// The text lists at most this many callers of one candidate. The JSON lists every caller.
const SHARED_CALLERS_SHOWN: usize = 5;

impl View for RefsResult {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(refs_text(self), RefsJson::of(self), None, wrap_json)
    }
}

// The unresolved-target notice and the skipped-file warning come first: each changes how every
// row below it reads. Only a scan that skipped nothing licenses the absence claim.
fn refs_text(result: &RefsResult) -> String {
    if let Some(key) = &result.data_key {
        return DATA_KEY_NO_FAN_IN.render((&one_line(&result.target), &one_line(key))) + "\n";
    }
    if let Some(reason) = &result.unresolved {
        return format!("{}\n", one_line(reason));
    }
    let complete_over = result.scanned.filter(|_| result.skipped.is_empty());
    let body = if result.shared_callers {
        shared_text(result)
    } else {
        groups_text(result, complete_over)
    };
    let pieces: Vec<String> = [notice(result), skipped_line(&result.skipped)]
        .into_iter()
        .filter(|piece| !piece.is_empty())
        .collect();
    if pieces.is_empty() {
        body
    } else {
        format!("{}\n\n{body}", pieces.join("\n\n"))
    }
}

// The fan-in of a target that named nothing is about its last name.
fn notice(result: &RefsResult) -> String {
    if result.target_resolved {
        return String::new();
    }
    let (base, _) = split_ordinal(&result.target);
    let after = |text: &'_ str, separator: &str| match text.rsplit_once(separator) {
        Some((_, tail)) => tail.to_owned(),
        None => text.to_owned(),
    };
    let name = after(&after(base, "::"), ".");
    let name = if name.is_empty() { base } else { &name };
    REFS_TARGET_UNRESOLVED.render((&result.target, name))
}

fn groups_text(result: &RefsResult, complete_over: Option<usize>) -> String {
    let groups = &result.groups;
    let named = result.definition.as_deref().unwrap_or(&result.target);
    if groups.rows.is_empty() {
        return absence_line(named, complete_over);
    }
    let mut blocks = vec![header(result, &one_line(named))];
    blocks.extend(groups.rows.iter().map(group_block));
    blocks.extend(footer_note(groups));
    blocks.join("\n\n") + "\n"
}

// One definition is named by its stable id. A name that several definitions share is counted,
// because its rows reference any one of them.
fn header(result: &RefsResult, named: &str) -> String {
    let total = result.groups.total;
    if result.target_resolved && result.definition.is_none() && result.definitions > 1 {
        return REFS_HEADER_SHARED.render((named, total, result.definitions));
    }
    REFS_HEADER.render((named, total))
}

fn absence_line(target: &str, complete_over: Option<usize>) -> String {
    let target = one_line(target);
    let line = match complete_over {
        Some(files) => REFS_ABSENT_COMPLETE.render((&target, files)),
        None => REFS_ABSENT.render(&target),
    };
    line + "\n"
}

fn group_block(group: &RefGroup) -> String {
    let mut lines = vec![group_header(group)];
    lines.extend(group.sites.iter().map(site_row));
    lines.join("\n")
}

// The header ends in a fact the tool wrote, so no file name can forge a whole line.
fn group_header(group: &RefGroup) -> String {
    let mut labelled = if group.tier_label.is_empty() {
        String::new()
    } else {
        format!(", {}", one_line(group.tier_label))
    };
    if !group.scope_checked {
        labelled.push_str(", locals not checked");
    }
    REFS_GROUP_HEADER.render((&one_line(&group.path), group.sites.len(), &labelled))
}

// The whole id, so a reader copies it to expand with no pattern to join.
fn site_row(site: &RefSite) -> String {
    let Some(stable_id) = &site.stable_id else {
        return format!("{ROW_INDENT}{} @{}", one_line(&site.enclosing), site.line);
    };
    let line = site.line.to_string();
    format!("{ROW_INDENT}{}", locator(stable_id, "", Some(&line)))
}

// The sites are cut before they are grouped, so a file can drop out whole.
fn footer_note(groups: &RefListing) -> Option<String> {
    let omitted = groups.omitted();
    if omitted == 0 {
        return None;
    }
    let files_omitted = groups.files_omitted();
    if files_omitted == 0 {
        return Some(REFS_UNLISTED.render((omitted, groups.limit)));
    }
    Some(REFS_UNLISTED_FILES.render((omitted, files_omitted, groups.limit)))
}

// Production candidates first, then the test-defined ones under their own heading.
fn shared_text(result: &RefsResult) -> String {
    let (listing, target) = (&result.shared, one_line(&result.target));
    if listing.rows().is_empty() {
        return format!("no symbols share callers with {target}\n");
    }
    let (from_tests, production): (Vec<&SharedCaller>, Vec<&SharedCaller>) =
        listing.rows().iter().partition(|row| row.in_tests);
    let mut lines = vec![format!("symbols sharing callers with {target}")];
    lines.extend(production.into_iter().flat_map(candidate_rows));
    if !from_tests.is_empty() {
        lines.push("  defined in tests (ranked below all production candidates):".to_owned());
        lines.extend(from_tests.into_iter().flat_map(candidate_rows));
    }
    if listing.omitted() > 0 {
        lines.push(REFS_CANDIDATES_UNLISTED.render((listing.omitted(), listing.limit())));
    }
    lines.join("\n") + "\n"
}

fn candidate_rows(row: &SharedCaller) -> Vec<String> {
    let line = row.line.to_string();
    let candidate = REFS_SHARED_CANDIDATE.render((
        &locator(&row.stable_id, "", Some(&line)),
        row.overlap,
        row.shared_files,
        &fixed(row.score, 3),
    ));
    let mut rows = vec![format!("  {candidate}")];
    let shown = row.callers.iter().take(SHARED_CALLERS_SHOWN);
    rows.extend(shown.map(|caller| {
        let site = file_site(&caller.path, caller.line);
        format!("      {}    {site}", one_line(&caller.qualname))
    }));
    let hidden = row.callers.len().saturating_sub(SHARED_CALLERS_SHOWN);
    if hidden > 0 {
        rows.push(REFS_CALLERS_UNLISTED.render((hidden, SHARED_CALLERS_SHOWN)));
    }
    rows
}

#[derive(Clone, Serialize)]
struct RefsJson<'a> {
    target: &'a str,
    target_resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    definition: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unresolved: Option<&'a str>,
    scanned: Option<usize>,
    total: usize,
    limit: usize,
    omitted: usize,
    files: usize,
    files_omitted: usize,
    groups: Vec<GroupJson<'a>>,
    shared_callers: SharedJson<'a>,
    skipped: Vec<SkippedJson<'a>>,
}

#[derive(Clone, Serialize)]
struct GroupJson<'a> {
    path: &'a str,
    count: usize,
    tier: &'a str,
    sites: Vec<SiteJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_checked: Option<bool>,
}

#[derive(Clone, Serialize)]
struct SiteJson<'a> {
    line: u32,
    enclosing: &'a str,
    stable_id: Option<&'a str>,
}

#[derive(Clone, Serialize)]
struct SharedJson<'a> {
    total: usize,
    limit: usize,
    omitted: usize,
    rows: Vec<SharedRowJson<'a>>,
}

#[derive(Clone, Serialize)]
struct SharedRowJson<'a> {
    stable_id: &'a str,
    path: &'a str,
    line: u32,
    overlap: usize,
    shared_files: usize,
    score: f64,
    defined_in_tests: bool,
    shared_callers: Vec<CallerJson<'a>>,
}

#[derive(Clone, Serialize)]
struct CallerJson<'a> {
    qualname: &'a str,
    path: &'a str,
    line: u32,
}

impl<'a> RefsJson<'a> {
    fn of(result: &'a RefsResult) -> Self {
        let groups = &result.groups;
        RefsJson {
            target: &result.target,
            target_resolved: result.target_resolved,
            definition: result.definition.as_deref(),
            data_key: result.data_key.as_deref(),
            unresolved: result.unresolved.as_deref(),
            scanned: result.scanned,
            total: groups.total,
            limit: groups.limit,
            omitted: groups.omitted(),
            files: groups.files,
            files_omitted: groups.files_omitted(),
            groups: groups.rows.iter().map(GroupJson::of).collect(),
            shared_callers: SharedJson::of(result),
            skipped: skipped_json(&result.skipped),
        }
    }
}

impl<'a> GroupJson<'a> {
    fn of(group: &'a RefGroup) -> Self {
        let sites = group.sites.iter().map(|site| SiteJson {
            line: site.line,
            enclosing: &site.enclosing,
            stable_id: site.stable_id.as_deref(),
        });
        GroupJson {
            path: &group.path,
            count: group.sites.len(),
            tier: group.tier,
            sites: sites.collect(),
            scope_checked: (!group.scope_checked).then_some(false),
        }
    }
}

impl<'a> SharedJson<'a> {
    fn of(result: &'a RefsResult) -> Self {
        let listing = &result.shared;
        SharedJson {
            total: listing.total(),
            limit: listing.limit(),
            omitted: listing.omitted(),
            rows: listing.rows().iter().map(SharedRowJson::of).collect(),
        }
    }
}

impl<'a> SharedRowJson<'a> {
    fn of(row: &'a SharedCaller) -> Self {
        let callers = row.callers.iter().map(|caller| CallerJson {
            qualname: &caller.qualname,
            path: &caller.path,
            line: caller.line,
        });
        SharedRowJson {
            stable_id: &row.stable_id,
            path: &row.path,
            line: row.line,
            overlap: row.overlap,
            shared_files: row.shared_files,
            score: round_half_even(row.score, 6),
            defined_in_tests: row.in_tests,
            shared_callers: callers.collect(),
        }
    }
}

impl Listing for RefsJson<'_> {
    fn items(&self) -> usize {
        self.groups.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.groups.truncate(kept);
        prefix
    }
}
