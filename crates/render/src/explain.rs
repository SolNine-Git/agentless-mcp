//! The explanation's two forms: the card with its tiered fan-out, fan-in and imports in the
//! text, and every group and listing in the JSON, trimmed at the fan-out groups.

use app::{EdgeRow, Explanation, ImportRow, TierGroup, cards_text, file_site, locator};
use kernel::Bounded;
use prompts::messages::{
    EXPLAIN_DEFINITIONS_UNLISTED, EXPLAIN_EDGES_UNLISTED, EXPLAIN_IMPORTERS_UNLISTED,
    EXPLAIN_IMPORTS_UNLISTED, EXPLAIN_TIE,
};
use prompts::{Count, Template};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::map::RationaleJson;
use crate::symbols::CardJson;
use crate::{Rendered, View};

impl View for Explanation {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(
            explanation_text(self),
            ExplainJson::of(self),
            None,
            wrap_json,
        )
    }
}

fn explanation_text(explanation: &Explanation) -> String {
    let Some(card) = &explanation.card else {
        return one_line(&explanation.message)
            .trim_end_matches('\n')
            .to_owned()
            + "\n";
    };
    let mut lines = Vec::new();
    if explanation.tied > 1 {
        let target = one_line(&explanation.target);
        lines.push(EXPLAIN_TIE.render((&target, explanation.tied)));
    }
    let card_text = cards_text(std::slice::from_ref(card));
    lines.push(card_text.trim_end_matches('\n').to_owned());
    if explanation.data_key {
        return format!(
            "{}\n\n{}\n",
            lines.join("\n"),
            one_line(&explanation.message)
        );
    }
    let alternatives = &explanation.alternatives;
    lines.extend(
        (alternatives.rows().iter()).map(|entry| format!("  also defined at {}", one_line(entry))),
    );
    lines.extend(cut_line(alternatives, &EXPLAIN_DEFINITIONS_UNLISTED));
    if !explanation.rationales.is_empty() {
        lines.push(String::new());
        lines.push("rationale".to_owned());
        lines.extend(explanation.rationales.iter().map(|node| {
            let line = node.line.to_string();
            let at = locator(&node.stable_id, &node.parent_id, Some(&line));
            let kind = one_line(node.kind).to_uppercase();
            format!("  {kind}  {}    {at}", one_line(&node.text))
        }));
    }
    lines.push(String::new());
    lines.push(tiers_text("references (fan-out)", &explanation.fan_out));
    lines.push(tiers_text("referenced by (fan-in)", &explanation.fan_in));
    lines.push(imports_text(explanation));
    lines.join("\n").trim_end_matches('\n').to_owned() + "\n"
}

// One side of the card, strongest tier first.
fn tiers_text(heading: &str, groups: &[TierGroup]) -> String {
    let total: usize = groups.iter().map(|group| group.total).sum();
    if total == 0 {
        return format!("{heading}: none\n");
    }
    let mut lines = vec![format!("{heading}: {total}")];
    for group in groups {
        lines.push(format!(
            "  {} ({})",
            one_line(group.tier_label),
            group.total
        ));
        lines.extend(group.rows.iter().map(edge_line));
        let omitted = group.total.saturating_sub(group.rows.len());
        if omitted > 0 {
            lines.push(EXPLAIN_EDGES_UNLISTED.render(omitted));
        }
    }
    lines.join("\n") + "\n"
}

fn edge_line(row: &EdgeRow) -> String {
    let line = row.line.to_string();
    format!(
        "    {} {}    {}",
        one_line(row.relation),
        one_line(&row.label),
        locator(&row.node, "", Some(&line))
    )
}

fn imports_text(explanation: &Explanation) -> String {
    let mut lines = vec!["imports".to_owned()];
    let declared = &explanation.imports_out;
    if declared.rows().is_empty() {
        lines.push("    declares  none resolved inside this repository".to_owned());
    } else {
        lines.extend(declared.rows().iter().map(|row| {
            let site = file_site(&row.path, row.line);
            format!(
                "    declares  {} -> {}    {site}",
                one_line(&row.module),
                one_line(&row.other)
            )
        }));
        lines.extend(cut_line(declared, &EXPLAIN_IMPORTS_UNLISTED));
    }
    let importers = &explanation.imports_in;
    if importers.rows().is_empty() {
        lines.push("    imported by  nothing in this repository".to_owned());
    } else {
        lines.extend(importers.rows().iter().map(|row| {
            let site = file_site(&row.path, row.line);
            format!("    imported by  {site}  as {}", one_line(&row.module))
        }));
        lines.extend(cut_line(importers, &EXPLAIN_IMPORTERS_UNLISTED));
    }
    lines.join("\n") + "\n"
}

fn cut_line<R>(listing: &Bounded<R>, unlisted: &Template<(Count, Count)>) -> Option<String> {
    let omitted = listing.omitted();
    (omitted > 0).then(|| unlisted.render((omitted, listing.limit())))
}

#[derive(Clone, Serialize)]
struct ExplainJson<'a> {
    target: &'a str,
    symbol: Option<CardJson<'a>>,
    message: &'a str,
    alternatives: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    alternatives_omitted: Option<usize>,
    fan_out: Vec<GroupJson<'a>>,
    fan_in: Vec<GroupJson<'a>>,
    imports: ImportsJson<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rationales: Vec<RationaleJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tied: Option<usize>,
}

#[derive(Clone, Serialize)]
struct GroupJson<'a> {
    tier: &'a str,
    total: usize,
    omitted: usize,
    rows: Vec<EdgeJson<'a>>,
}

#[derive(Clone, Serialize)]
struct EdgeJson<'a> {
    node: &'a str,
    label: &'a str,
    path: &'a str,
    line: u32,
    relation: &'a str,
}

#[derive(Clone, Serialize)]
struct ImportsJson<'a> {
    declared: ImportListingJson<'a>,
    importers: ImportListingJson<'a>,
}

#[derive(Clone, Serialize)]
struct ImportListingJson<'a> {
    total: usize,
    limit: usize,
    omitted: usize,
    rows: Vec<ImportJson<'a>>,
}

#[derive(Clone, Serialize)]
struct ImportJson<'a> {
    path: &'a str,
    line: u32,
    module: &'a str,
    other: &'a str,
}

impl<'a> ExplainJson<'a> {
    fn of(explanation: &'a Explanation) -> Self {
        ExplainJson {
            target: &explanation.target,
            symbol: explanation.card.as_ref().map(CardJson::of),
            message: &explanation.message,
            alternatives: explanation.alternatives.rows(),
            alternatives_omitted: Some(explanation.alternatives.omitted()).filter(|&left| left > 0),
            fan_out: explanation.fan_out.iter().map(GroupJson::of).collect(),
            fan_in: explanation.fan_in.iter().map(GroupJson::of).collect(),
            imports: ImportsJson {
                declared: ImportListingJson::of(&explanation.imports_out),
                importers: ImportListingJson::of(&explanation.imports_in),
            },
            rationales: explanation
                .rationales
                .iter()
                .map(RationaleJson::of)
                .collect(),
            tied: (explanation.tied > 1).then_some(explanation.tied),
        }
    }
}

impl<'a> GroupJson<'a> {
    fn of(group: &'a TierGroup) -> Self {
        let rows = group.rows.iter().map(|row| EdgeJson {
            node: &row.node,
            label: &row.label,
            path: &row.path,
            line: row.line,
            relation: row.relation,
        });
        GroupJson {
            tier: group.tier,
            total: group.total,
            omitted: group.total.saturating_sub(group.rows.len()),
            rows: rows.collect(),
        }
    }
}

impl<'a> ImportListingJson<'a> {
    fn of(listing: &'a Bounded<ImportRow>) -> Self {
        let rows = listing.rows().iter().map(|row| ImportJson {
            path: &row.path,
            line: row.line,
            module: &row.module,
            other: &row.other,
        });
        ImportListingJson {
            total: listing.total(),
            limit: listing.limit(),
            omitted: listing.omitted(),
            rows: rows.collect(),
        }
    }
}

// The ceiling drops whole fan-out groups, the list the command line names.
impl Listing for ExplainJson<'_> {
    fn items(&self) -> usize {
        self.fan_out.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.fan_out.truncate(kept);
        prefix
    }
}
