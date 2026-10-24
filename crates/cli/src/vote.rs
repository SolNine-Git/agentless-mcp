//! The `vote` step: a verdicts file read and ranked, the ranking on stdout, and the run it came
//! from on stderr.

use std::path::Path;

use app::{BaselineStatus, Services};
use patch::Tier;

use crate::Streams;
use crate::caller::{named_file, shown_path};
use crate::exit::{EXIT_USAGE, code_for, domain_code, fail};
use crate::invocation::Vote;
use crate::steps::answer_with;

const UNVERIFIED: &str = "agentless-mcp: this ranking comes from an UNVERIFIED run -- the test \
    command did not pass on unpatched HEAD, so no candidate was evaluated.\n";

// A ranking that a caller can act on exits 0: a verified baseline and a tier that ranked.
pub(crate) fn run(call: &Vote, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let source = shown_path(&call.verdicts.text);
    let text = match named_file(Path::new(&call.verdicts.raw), &source, services) {
        Ok(text) => text,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let vote = match app::vote(&text) {
        Ok(vote) => vote,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let out = if call.json {
        render::document_json(&vote.report)
    } else {
        Ok(render::vote_text(&vote.report))
    };
    let mut notes = [
        format!("# agentless-mcp vote over {source}"),
        format!("# test-cmd: {}", vote.verdicts.test_cmd),
        format!("# {}", render::vote_summary(&vote.report)),
    ]
    .map(|line| render::note_text(&line))
    .concat();
    let verified = vote.verdicts.baseline == BaselineStatus::Ok;
    if !verified {
        notes.push_str(UNVERIFIED);
    }
    let ranked = verified && vote.report.tier != Tier::None;
    answer_with(streams, out, &notes, domain_code(ranked))
}
