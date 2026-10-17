//! The labels of a filled labeling file, counted per check, with each row matched to the log.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use ::text::CodePointString;

use crate::report::classify::Classified;
use crate::report::csv::{Cell, DictReader, Row};
use crate::report::sample::{CHECK, FINDING_ID, LABEL};

const REQUIRED: [&str; 3] = [FINDING_ID, CHECK, LABEL];
const CORRECT: &str = "correct";
const WRONG: &str = "wrong";

// The labels one check received in a filled labeling file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LabelCount {
    pub(crate) correct: usize,
    pub(crate) wrong: usize,
    pub(crate) unlabeled: usize,
}

pub(crate) type Labels = HashMap<CodePointString, LabelCount>;

pub(crate) fn read_labels(path: &str, classified: &[Classified<'_>]) -> Result<Labels, String> {
    let applied: HashMap<&CodePointString, &Classified<'_>> = classified
        .iter()
        .filter(|item| item.class.applied())
        .map(|item| (&item.finding_id, item))
        .collect();
    let unreadable = |error: String| format!("cannot read {path}: {error}");
    let bytes =
        fs::read(path).map_err(|error| unreadable(io::os_error_text(&error, Path::new(path))))?;
    let mut reader = DictReader::new(&bytes);
    let names = reader.fieldnames().map_err(unreadable)?;
    check_header(names, path)?;
    let mut labels = Labels::new();
    let mut seen = HashSet::new();
    while let Some(row) = reader.next_row() {
        let row = row.map_err(unreadable)?;
        let at = format!("{path}:{}", reader.line_num());
        let (check, label) = label_row(&row, &applied, &mut seen, &at)?;
        let count = labels.entry(check).or_default();
        match label.as_str() {
            CORRECT => count.correct += 1,
            WRONG => count.wrong += 1,
            _ => count.unlabeled += 1,
        }
    }
    Ok(labels)
}

fn check_header(names: Option<&[String]>, path: &str) -> Result<(), String> {
    let Some(names) = names else {
        return Err(format!("{path} is empty"));
    };
    let missing: Vec<&str> = REQUIRED
        .into_iter()
        .filter(|required| !names.iter().any(|name| name == required))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!("{path}: the header lacks {}", missing.join(", ")))
}

fn label_row(
    row: &Row,
    applied: &HashMap<&CodePointString, &Classified<'_>>,
    seen: &mut HashSet<String>,
    at: &str,
) -> Result<(CodePointString, String), String> {
    let [finding_id, check, label] = REQUIRED.map(|name| text_of(row, name));
    let (Some(finding_id), Some(check), Some(label)) = (finding_id, check, label) else {
        return Err(format!("{at}: the row has fewer columns than the header"));
    };
    let label = ::text::strip(label);
    if ![CORRECT, WRONG, ""].contains(&label) {
        let shown = CodePointString::from(label).quoted();
        return Err(format!(
            "{at}: label {shown} is not correct, wrong, or empty"
        ));
    }
    if !seen.insert(finding_id.to_owned()) {
        return Err(format!("{at}: {finding_id} appears twice"));
    }
    let Some(item) = applied.get(&CodePointString::from(finding_id)) else {
        return Err(format!(
            "{at}: {finding_id} is not an applied finding in the log"
        ));
    };
    let logged = &item.finding.check;
    if logged.as_str() != Some(check) {
        let logged = crate::text::lossy(logged);
        return Err(format!(
            "{at}: {finding_id} is a {logged} finding, not {check}"
        ));
    }
    Ok((logged.clone(), label.to_owned()))
}

// The text of the column `name`, or `None` for a column the row lacks.
fn text_of<'r>(row: &'r Row, name: &str) -> Option<&'r str> {
    let cell = row
        .iter()
        .find(|(key, _)| key.as_deref() == Some(name))
        .map(|(_, cell)| cell);
    match cell {
        Some(Cell::Text(text)) => Some(text),
        _ => None,
    }
}
