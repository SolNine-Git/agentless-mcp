//! The warmup report as text: the grammar release and cache, then one row per language.

use app::{GRAMMAR_CACHE, WarmupReport, WarmupRow};

/// Returns the warmup report: the release line, then each language's tier, ABI, warm state,
/// probe result and detail.
#[must_use]
pub fn warmup_text(report: &WarmupReport) -> String {
    let release = format!("pack {}  cache {GRAMMAR_CACHE}", report.pack_version);
    let rows = report.rows.iter().map(row_line);
    let lines: Vec<String> = std::iter::once(release).chain(rows).collect();
    lines.join("\n") + "\n"
}

fn row_line(row: &WarmupRow) -> String {
    let line = match row {
        WarmupRow::Linked(capability) => {
            let abi = capability
                .abi
                .filter(|&abi| abi > 0)
                .map_or_else(|| "-".to_owned(), |abi| abi.to_string());
            format!(
                "  {:<12} tier={} abi={abi} warmed={} probe={} {}",
                capability.name,
                capability.tier,
                truth(capability.warmed),
                truth(capability.probe_ok),
                capability.detail
            )
        }
        WarmupRow::Unknown(name) => format!(
            "  {name:<12} tier={} abi=- warmed=False probe=False no linked grammar is named {}",
            row.tier(),
            text::CodePointString::from(name.as_str()).quoted()
        ),
    };
    line.trim_end().to_owned()
}

// A row spells a truth value True or False.
fn truth(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}
