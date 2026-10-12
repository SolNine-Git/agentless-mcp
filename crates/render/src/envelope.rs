//! The envelope: the receipt around a body, and the token ceiling over the whole answer.

use json::{JsonForm, to_text};
use prompts::envelope::{
    CEILING_TRUNCATION, JSON_CEILING_OVERSIZED_ITEM, JSON_CEILING_TRIMMED, NOTICE, RECEIPT_HEADER,
};
use serde::Serialize;
use text::splitlines;

use crate::Error;
use crate::count::{chars_to_tokens, tokens};
use crate::receipt::{BLOCK_SHARE, Fields, Receipt, bounded_warnings, warning_line};

// Room for the truncation note itself, so the note cannot push the answer over the ceiling.
const MARKER_ALLOWANCE: usize = 64;

pub(crate) trait Listing: Serialize {
    fn items(&self) -> usize;
    fn prefix(&self, kept: usize) -> Self;
}

// The header and the config warnings each get at most one share of the ceiling before the body
// gets what is left, so no repository can spend the ceiling on the envelope.
pub(crate) fn wrap(receipt: &Receipt<'_>, body: &str, max_tokens: usize) -> String {
    let header = header(receipt, max_tokens / BLOCK_SHARE);
    let warnings: String = bounded_warnings(&receipt.warnings, max_tokens)
        .iter()
        .map(|warning| warning_line(warning) + "\n")
        .collect();
    let budget = max_tokens
        .saturating_sub(tokens(&header))
        .saturating_sub(tokens(&warnings))
        .saturating_sub(MARKER_ALLOWANCE);
    let (kept, dropped) = fit(body, budget);
    let mut wrapped = header;
    wrapped.push_str(kept);
    if !kept.is_empty() && !kept.ends_with('\n') {
        wrapped.push('\n');
    }
    wrapped.push_str(&warnings);
    if dropped > 0 {
        let total = splitlines(body, false).len();
        wrapped.push_str(&CEILING_TRUNCATION.render((dropped, total, max_tokens)));
        wrapped.push('\n');
    }
    wrapped
}

// The marker line survives a clamp that drops every other line: losing it is the worse failure.
fn header(receipt: &Receipt<'_>, budget: usize) -> String {
    let mut block = String::new();
    for line in receipt.tool_lines() {
        block.push_str(&line);
        block.push('\n');
    }
    match fit(&block, budget) {
        ("", _) => format!("{RECEIPT_HEADER}\n"),
        (kept, _) => kept.to_owned(),
    }
}

// The longest whole-line prefix of `body` within `budget`, and the count of lines it drops.
fn fit(body: &str, budget: usize) -> (&str, usize) {
    if budget == 0 {
        return ("", splitlines(body, false).len());
    }
    if tokens(body) <= budget {
        return (body, 0);
    }
    let lines = splitlines(body, true);
    let (mut chars, mut bytes, mut kept) = (0, 0, 0);
    for line in &lines {
        let next = chars + line.chars().count();
        if chars_to_tokens(next) > budget {
            break;
        }
        (chars, bytes, kept) = (next, bytes + line.len(), kept + 1);
    }
    (&body[..bytes], lines.len() - kept)
}

// A JSON document cannot be cut mid-way, so the ceiling drops whole items and says so. An
// oversized first item is returned whole: an empty list would read as "nothing found".
pub(crate) fn wrap_json<P: Listing>(
    receipt: &Receipt<'_>,
    payload: &P,
    max_tokens: usize,
) -> Result<String, Error> {
    let fields = receipt.fields(max_tokens);
    let whole = document(&fields, payload, None)?;
    if tokens(&whole) <= max_tokens {
        return Ok(whole);
    }
    let total = payload.items();
    let fitted = fit_items(&fields, payload, max_tokens)?;
    let oversized = fitted == 0 && total > 0;
    let kept = if oversized { 1 } else { fitted };
    let truncated = Trimmed::new(max_tokens, kept, total, oversized);
    document(&fields, &payload.prefix(kept), Some(truncated))
}

fn fit_items<P: Listing>(
    fields: &Fields<'_>,
    payload: &P,
    max_tokens: usize,
) -> Result<usize, Error> {
    let total = payload.items();
    let (mut low, mut high) = (0, total);
    while low < high {
        let middle = (low + high).div_ceil(2);
        let truncated = Trimmed::new(max_tokens, middle, total, false);
        let probe = document(fields, &payload.prefix(middle), Some(truncated))?;
        if tokens(&probe) <= max_tokens {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Ok(low)
}

#[derive(Serialize)]
struct Document<'a, P> {
    receipt: &'a Fields<'a>,
    notice: &'static str,
    #[serde(flatten)]
    payload: &'a P,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<Trimmed>,
}

#[derive(Serialize)]
struct Trimmed {
    reason: String,
    token_ceiling: usize,
    shown: usize,
    total: usize,
}

impl Trimmed {
    fn new(max_tokens: usize, shown: usize, total: usize, oversized: bool) -> Self {
        let reason = if oversized {
            JSON_CEILING_OVERSIZED_ITEM
        } else {
            JSON_CEILING_TRIMMED
        };
        Self {
            reason: reason.render(max_tokens),
            token_ceiling: max_tokens,
            shown,
            total,
        }
    }
}

fn document<P: Serialize>(
    receipt: &Fields<'_>,
    payload: &P,
    truncated: Option<Trimmed>,
) -> Result<String, Error> {
    let document = Document {
        receipt,
        notice: NOTICE,
        payload,
        truncated,
    };
    let mut text = to_text(&document, JsonForm::Indented)?;
    text.push('\n');
    Ok(text)
}
