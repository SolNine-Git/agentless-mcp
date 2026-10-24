//! The steps of the two commands that read no repository: `guide` and `warmup`.

use std::os::unix::ffi::OsStrExt;

use app::{Services, WarmupRequest};
use text::CodePointString;

use super::ReadArgs;
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK, EXIT_USAGE, fail, write_answer};

pub(super) fn guide(args: &ReadArgs<'_>, _: &Services, streams: &mut Streams<'_>) -> u8 {
    let Some(section) = args.text("section") else {
        return write_answer(streams, &terminated(app::guide_text()), "", EXIT_OK);
    };
    if let Some(text) = app::section_text(&section.text) {
        return write_answer(streams, &terminated(text), "", EXIT_OK);
    }
    let name = CodePointString::from_utf8_escaped(section.raw.as_bytes());
    fail(streams.err, &app::unknown_section(&name), EXIT_USAGE)
}

// The rows go to stdout whatever the outcome. A degraded grammar fails the run when the caller
// named it, or when it is tier 1; a degraded tier-2 grammar gets a note.
pub(super) fn warmup(args: &ReadArgs<'_>, _: &Services, streams: &mut Streams<'_>) -> u8 {
    let languages = args.list("languages").iter().map(|arg| arg.text.clone());
    let report = app::warmup(&WarmupRequest {
        languages: languages.collect(),
    });
    let rows = render::warmup_text(&report);
    let fatal = report.fatal();
    let degraded = report.degraded().count();
    let (err, code) = if fatal > 0 {
        let message = format!("{fatal} grammars degraded; see the rows above");
        (format!("agentless-mcp: {message}\n"), EXIT_DOMAIN)
    } else if degraded > 0 {
        let note = format!(
            "agentless-mcp: {degraded} tier-2 grammars degraded; those languages are \
             unavailable, the rest are unaffected\n"
        );
        (note, EXIT_OK)
    } else {
        (String::new(), EXIT_OK)
    };
    write_answer(streams, &rows, &err, code)
}

// An answer ends with exactly one line break of its own.
fn terminated(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}
