//! The envelope: the receipt around a body, and the token ceiling over the whole answer.

use json::{JsonForm, to_text};
use prompts::envelope::{
    CEILING_TRUNCATION, JSON_CEILING_OVERSIZED_ITEM, JSON_CEILING_TRIMMED, NOTICE, RECEIPT_HEADER,
};
use serde::Serialize;
use text::splitlines;

use crate::Error;
use crate::count::Ceiling;
use crate::receipt::{BLOCK_SHARE, Fields, Receipt, bounded_warnings, warning_line};

// Room for the truncation note itself, so the note cannot push the answer over the ceiling.
const MARKER_ALLOWANCE: usize = 64;

// A payload whose bulk is one list. `prefix` keeps the first `kept` items of that list, wherever
// in the payload it sits.
pub(crate) trait Listing: Serialize {
    fn items(&self) -> usize;
    fn prefix(&self, kept: usize) -> Self;
}

// The header and the config warnings each get at most one share of the ceiling before the body
// gets what is left, so no repository can spend the ceiling on the envelope.
pub(crate) fn wrap(receipt: &Receipt<'_>, body: &str, ceiling: &Ceiling<'_>) -> String {
    let header = header(receipt, ceiling);
    let warnings: String = bounded_warnings(&receipt.warnings, ceiling)
        .iter()
        .map(|warning| warning_line(warning) + "\n")
        .collect();
    let budget = ceiling
        .tokens
        .saturating_sub(ceiling.count(&header))
        .saturating_sub(ceiling.count(&warnings))
        .saturating_sub(MARKER_ALLOWANCE);
    let (kept, dropped) = fit(body, ceiling, budget);
    let mut wrapped = header;
    wrapped.push_str(kept);
    if !kept.is_empty() && !kept.ends_with('\n') {
        wrapped.push('\n');
    }
    wrapped.push_str(&warnings);
    if dropped > 0 {
        let total = splitlines(body, false).len();
        wrapped.push_str(&CEILING_TRUNCATION.render((dropped, total, ceiling.tokens)));
        wrapped.push('\n');
    }
    wrapped
}

// The marker line survives a clamp that drops every other line: losing it is the worse failure.
fn header(receipt: &Receipt<'_>, ceiling: &Ceiling<'_>) -> String {
    let mut block = String::new();
    for line in receipt.tool_lines() {
        block.push_str(&line);
        block.push('\n');
    }
    match fit(&block, ceiling, ceiling.tokens / BLOCK_SHARE) {
        ("", _) => format!("{RECEIPT_HEADER}\n"),
        (kept, _) => kept.to_owned(),
    }
}

// The longest whole-line prefix of `body` within `budget`, and the count of lines it drops. A
// count need not add up over lines, so the search measures each candidate prefix whole.
fn fit<'b>(body: &'b str, ceiling: &Ceiling<'_>, budget: usize) -> (&'b str, usize) {
    if budget == 0 {
        return ("", splitlines(body, false).len());
    }
    if ceiling.count(body) <= budget {
        return (body, 0);
    }
    let ends: Vec<usize> = splitlines(body, true)
        .iter()
        .scan(0, |end, line| {
            *end += line.len();
            Some(*end)
        })
        .collect();
    let prefix = |lines: usize| &body[..lines.checked_sub(1).map_or(0, |last| ends[last])];
    let (mut low, mut high) = (0, ends.len());
    while low < high {
        let middle = (low + high).div_ceil(2);
        if ceiling.count(prefix(middle)) <= budget {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    (prefix(low), ends.len() - low)
}

// A JSON document cannot be cut mid-way, so the ceiling drops whole items and says so. An
// oversized first item is returned whole: an empty list would read as "nothing found".
pub(crate) fn wrap_json<P: Listing>(
    receipt: &Receipt<'_>,
    payload: &P,
    ceiling: &Ceiling<'_>,
) -> Result<String, Error> {
    let fields = receipt.fields(ceiling);
    let whole = document(&fields, payload, None)?;
    if ceiling.count(&whole) <= ceiling.tokens {
        return Ok(whole);
    }
    let total = payload.items();
    let fitted = fit_items(&fields, payload, ceiling)?;
    let oversized = fitted == 0 && total > 0;
    let kept = if oversized { 1 } else { fitted };
    let truncated = Trimmed::new(ceiling.tokens, kept, total, oversized);
    document(&fields, &payload.prefix(kept), Some(truncated))
}

fn fit_items<P: Listing>(
    fields: &Fields<'_>,
    payload: &P,
    ceiling: &Ceiling<'_>,
) -> Result<usize, Error> {
    let total = payload.items();
    let (mut low, mut high) = (0, total);
    while low < high {
        let middle = (low + high).div_ceil(2);
        let truncated = Trimmed::new(ceiling.tokens, middle, total, false);
        let probe = document(fields, &payload.prefix(middle), Some(truncated))?;
        if ceiling.count(&probe) <= ceiling.tokens {
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
