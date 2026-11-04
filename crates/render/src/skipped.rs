//! The files a scan passed over: one warning line in the text, and every entry in the JSON.

use app::Skipped;
use facts::is_data_language;
use prompts::messages::SCAN_SKIPPED_FILES;
use serde::Serialize;
use syntax::Language;
use text::one_line;

// The text names at most this many skipped files. The JSON names every one.
const SKIPPED_FILES_SHOWN: usize = 5;

#[derive(Clone, Serialize)]
pub(crate) struct SkippedJson<'a> {
    path: &'a str,
    reason: &'a str,
}

pub(crate) fn skipped_json(skipped: &[Skipped]) -> Vec<SkippedJson<'_>> {
    skipped
        .iter()
        .map(|entry| SkippedJson {
            path: &entry.path,
            reason: &entry.reason,
        })
        .collect()
}

// The warning line, or nothing when the scan passed over no code file. Each path carries its
// reason. A data file's keys are not definitions, so a skipped data file reads as no loss.
pub(crate) fn skipped_line(skipped: &[Skipped]) -> String {
    let skipped: Vec<&Skipped> = (skipped.iter())
        .filter(|entry| !Language::from_path(&entry.path).is_some_and(is_data_language))
        .collect();
    if skipped.is_empty() {
        return String::new();
    }
    let shown = skipped.iter().take(SKIPPED_FILES_SHOWN);
    let shown: Vec<String> = shown
        .map(|entry| format!("{} ({})", one_line(&entry.path), one_line(&entry.reason)))
        .collect();
    let hidden = skipped.len() - shown.len();
    let listed = if hidden > 0 {
        format!("{}; ... {hidden} more", shown.join("; "))
    } else {
        shown.join("; ")
    };
    SCAN_SKIPPED_FILES.render((skipped.len(), &listed))
}
