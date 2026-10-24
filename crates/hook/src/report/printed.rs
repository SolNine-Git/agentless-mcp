//! The printed report: the log and transcript lines, each check's classes, rates and verdict, the
//! closing condition, and the notes.

use ::text::CodePointString;
use checks::{COMPLEXITY_DELTA, Match};

use crate::record::Outcome;
use crate::report::classify::{Class, Classified, checks};
use crate::report::labels::{LabelCount, Labels};
use crate::report::log::Record;
use crate::report::transcript::Transcripts;
use crate::text::lossy;

const MIN_APPLIED: usize = 50;
// Precision at or above 9/10, compared in integers so 27 of 30 meets the bar exactly.
const PRECISION_NUMERATOR: usize = 9;
const PRECISION_DENOMINATOR: usize = 10;
const TOP_REASONS: usize = 5;
const CONDITION_3: &str = "Flip condition 3 (no change to a check's rules or message since its \
                           trial started) is not in the log. Confirm it before you set a check's \
                           erosion_gate entry to block.";
const SECONDS_PER_DAY: i64 = 86_400;
const MICROS: f64 = 1e6;

pub(crate) fn report_text(
    records: &[Record],
    transcripts: &Transcripts,
    classified: &[Classified<'_>],
    labels: Option<&Labels>,
    notes: &[String],
) -> Result<String, String> {
    let mut lines = vec!["Erosion gate trial report".to_owned()];
    lines.extend(log_lines(records)?);
    lines.extend(transcript_lines(transcripts));
    let checks = checks(classified);
    if checks.is_empty() {
        lines.push("No findings in the log.".to_owned());
    }
    for check in &checks {
        lines.push(String::new());
        lines.extend(check_lines(check, classified, labels));
    }
    lines.extend([String::new(), CONDITION_3.to_owned()]);
    lines.extend(notes.iter().cloned());
    Ok(lines.join("\n") + "\n")
}

fn log_lines(records: &[Record]) -> Result<Vec<String>, String> {
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        return Ok(vec!["Log: 0 records.".to_owned()]);
    };
    let (from, to) = (when(first.ts)?, when(last.ts)?);
    let mut lines = vec![
        format!("Log: {} records, from {from} to {to}.", records.len()),
        outcomes_line(records),
    ];
    lines.extend(degraded_reasons_line(records));
    lines.extend(binaries_line(records));
    Ok(lines)
}

fn outcomes_line(records: &[Record]) -> String {
    let counts = Outcome::ALL.map(|outcome| {
        let count = records
            .iter()
            .filter(|record| record.outcome == outcome)
            .count();
        format!("{} {count}", outcome.name())
    });
    format!("Outcomes: {}.", counts.join(", "))
}

// Each non-empty text with its count, in first-seen order.
fn tally<'a>(
    values: impl Iterator<Item = &'a CodePointString>,
) -> Vec<(&'a CodePointString, usize)> {
    let mut counts: Vec<(&CodePointString, usize)> = Vec::new();
    for value in values.filter(|value| value.as_str() != Some("")) {
        if let Some(slot) = counts.iter().position(|(seen, _)| *seen == value) {
            counts[slot].1 += 1;
        } else {
            counts.push((value, 1));
        }
    }
    counts
}

// The commonest first; a tie keeps first-seen order.
fn degraded_reasons_line(records: &[Record]) -> Option<String> {
    let reasons = records
        .iter()
        .filter(|record| record.outcome == Outcome::Degraded)
        .filter_map(|record| record.reason.as_ref());
    let mut counts = tally(reasons);
    if counts.is_empty() {
        return None;
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let common: Vec<String> = counts
        .iter()
        .take(TOP_REASONS)
        .map(|(reason, count)| format!("{} ({count})", lossy(reason)))
        .collect();
    Some(format!(
        "Commonest degraded reasons: {}.",
        common.join(", ")
    ))
}

fn binaries_line(records: &[Record]) -> Option<String> {
    let counts = tally(records.iter().filter_map(|record| record.binary.as_ref()));
    if counts.is_empty() {
        return None;
    }
    let seen: Vec<String> = counts
        .iter()
        .map(|(binary, count)| format!("{} ({count} records)", lossy(binary)))
        .collect();
    Some(format!("Binaries: {}.", seen.join(", ")))
}

fn transcript_lines(transcripts: &Transcripts) -> Vec<String> {
    let missing = transcripts
        .values()
        .filter(|transcript| transcript.results.is_none())
        .count();
    let mut lines = vec![format!(
        "Transcripts: {} named, {missing} missing.",
        transcripts.len()
    )];
    for (path, transcript) in transcripts {
        let count = transcript.unreadable_lines;
        if count > 0 {
            let path = lossy(path);
            lines.push(format!("Transcript {path} has {count} unreadable lines."));
        }
    }
    lines
}

fn check_lines(
    check: &CodePointString,
    classified: &[Classified<'_>],
    labels: Option<&Labels>,
) -> Vec<String> {
    let items: Vec<&Classified<'_>> = classified
        .iter()
        .filter(|item| item.finding.check == *check)
        .collect();
    let count = |class: Class| items.iter().filter(|item| item.class == class).count();
    let classes = Class::ALL.map(|class| format!("{} {}", class.name(), count(class)));
    let applied: usize = Class::APPLIED.into_iter().map(count).sum();
    let labeled = labels.map(|labels| labels_for(labels, check));
    let mut lines = vec![
        format!("{}: {} findings", lossy(check), items.len()),
        format!("  {}", classes.join(", ")),
        format!("  applied {applied}"),
    ];
    if check.as_str() == Some(COMPLEXITY_DELTA) {
        lines.push(match_pass_line(&items));
    }
    lines.push(false_positive_line(
        count(Class::ActedOn),
        count(Class::NotActedOn),
    ));
    lines.extend(labeled.map(precision_line));
    lines.push(format!("  flip: {}", verdict(applied, labeled)));
    lines
}

// A check that no row of a filled file names has no labels.
fn labels_for(labels: &Labels, check: &CodePointString) -> LabelCount {
    match labels.get(check) {
        Some(count) => *count,
        None => LabelCount::default(),
    }
}

fn match_pass_line(items: &[&Classified<'_>]) -> String {
    let passes = Match::ALL.map(|pass| {
        let count = items
            .iter()
            .filter(|item| item.finding.via == Some(pass))
            .count();
        format!("{} {count}", pass.as_str())
    });
    format!("  match pass: {}", passes.join(", "))
}

fn false_positive_line(acted_on: usize, not_acted_on: usize) -> String {
    let observed = acted_on + not_acted_on;
    let rate = ratio(not_acted_on, observed).map_or_else(|| "n/a".to_owned(), two_places);
    format!("  effective false-positive rate: {rate} ({not_acted_on} of {observed} observed)")
}

fn precision_line(count: LabelCount) -> String {
    let labeled = count.correct + count.wrong;
    let precision = ratio(count.correct, labeled).map_or_else(|| "n/a".to_owned(), two_places);
    let total = labeled + count.unlabeled;
    let correct = count.correct;
    format!(
        "  labeled precision: {precision} ({correct} of {labeled} correct), {labeled} of {total} \
         labeled"
    )
}

// One check's flip verdict on conditions 1 and 2; condition 3 stays with the user.
pub(crate) fn verdict(applied: usize, labels: Option<LabelCount>) -> String {
    if applied < MIN_APPLIED {
        return format!("does not qualify: {applied} applied findings, under {MIN_APPLIED}.");
    }
    let Some(labels) = labels else {
        return format!(
            "condition 1 holds ({applied} applied findings). Label a sample with --sample-out \
             and --labels to test condition 2."
        );
    };
    let labeled = labels.correct + labels.wrong;
    let Some(precision) = ratio(labels.correct, labeled).filter(|_| labels.unlabeled == 0) else {
        let total = labeled + labels.unlabeled;
        return format!("does not qualify: {labeled} of {total} sampled findings are labeled.");
    };
    let precision = two_places(precision);
    if labels.correct * PRECISION_DENOMINATOR < labeled * PRECISION_NUMERATOR {
        return format!("does not qualify: precision {precision}, under 0.90.");
    }
    format!(
        "qualifies on conditions 1 and 2 ({applied} applied findings, precision {precision} \
         over {labeled} labeled). Confirm condition 3 before you set erosion_gate to block."
    )
}

// The true quotient of two counts, or `None` for a zero divisor.
fn ratio(part: usize, whole: usize) -> Option<f64> {
    if whole == 0 {
        return None;
    }
    let float = |count: usize| count.to_string().parse::<f64>().ok();
    Some(float(part)? / float(whole)?)
}

fn two_places(value: f64) -> String {
    json::fixed(value, 2)
}

// The UTC minute of a log time, or the last line of the reference's traceback when the time has
// no date (ledger deviation).
pub(crate) fn when(ts: f64) -> Result<String, String> {
    if ts.is_nan() {
        return Err("ValueError: Invalid value NaN (not a number)".to_owned());
    }
    let seconds = whole_seconds(ts)?;
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil(days);
    // gmtime holds the year less 1900 in a C int, so a year outside that range is EOVERFLOW.
    if !(i64::from(i32::MIN) + 1900..=i64::from(i32::MAX) + 1900).contains(&year) {
        return Err("OSError: [Errno 75] Value too large for defined data type".to_owned());
    }
    if !(1..=9999).contains(&year) {
        return Err(format!("ValueError: year {year} is out of range"));
    }
    let (hour, minute) = (of_day / 3600, of_day % 3600 / 60);
    Ok(format!(
        "{year}-{month:02}-{day:02} {hour:02}:{minute:02} UTC"
    ))
}

// The whole seconds of `ts` after its microseconds are rounded half to even, which can carry.
fn whole_seconds(ts: f64) -> Result<i64, String> {
    let whole = ts.trunc();
    let micros = ((ts - whole) * MICROS).round_ties_even();
    let seconds = if micros >= MICROS {
        whole + 1.0
    } else if micros < 0.0 {
        whole - 1.0
    } else {
        whole
    };
    // The range of the C time type, as floats: from -2**63, up to and without 2**63.
    let limit = 9_223_372_036_854_775_808.0_f64;
    if !(-limit..limit).contains(&seconds) {
        return Err("OverflowError: timestamp out of range for platform time_t".to_owned());
    }
    Ok(format!("{seconds:.0}").parse().unwrap_or(i64::MAX))
}

// The proleptic Gregorian date of a day count from 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
