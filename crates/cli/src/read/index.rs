//! The step of `index`: build or refresh the repository's cache, then print the summary with its
//! first error and warning rows, or the report as JSON. Any per-file error exits 1.

use std::path::Path;

use app::{IndexRequest, Services};
use render::{Receipt, index_json, index_text, one_line};

use super::ReadArgs;
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK, code_for, fail, report};
use crate::steps::{answer, diagnostics};

// The index reads no cache, so its receipt names none.
const NO_CACHE: &str = "none";

pub(super) fn index(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let repo = flags.repo.as_deref().map(Path::new);
    let repo = match app::resolve(services, repo) {
        Ok(repo) => repo,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let enclosing = services.git.root(&repo.root);
    let snapshot = services.git.snapshot(&repo.root, enclosing.as_deref());
    if !snapshot.note.is_empty() {
        report(
            streams.err,
            &format!("agentless-mcp: {}\n", one_line(&snapshot.note)),
        );
    }
    let request = IndexRequest {
        force: args.flag("force"),
        max_cache_bytes: services.max_cache_bytes.clone(),
    };
    let outcome = app::index(services, &repo, &snapshot, &request);
    diagnostics(streams, &outcome.diagnostics);
    let indexed = match outcome.value {
        Ok(indexed) => indexed,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let code = if indexed.errors() == 0 {
        EXIT_OK
    } else {
        EXIT_DOMAIN
    };
    let rendered = if flags.json {
        let receipt = Receipt::new(&repo, &snapshot, NO_CACHE).counted_by(&*services.counter);
        index_json(&receipt, &indexed)
    } else {
        Ok(index_text(&indexed))
    };
    answer(streams, rendered, code)
}
