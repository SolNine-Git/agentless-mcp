//! The step of `index`: build or refresh the repository's cache, then print the summary with its
//! first error and warning rows, or the report as JSON. Any per-file error exits 1.

use app::{IndexRequest, Services};
use render::{index_json, index_text};

use super::ReadArgs;
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK, code_for, fail};
use crate::steps::{answer, diagnostics, resolved};

pub(super) fn index(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let found = match resolved(flags.repo.as_deref(), app::resolve, services, streams) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let request = IndexRequest {
        force: args.flag("force"),
        max_cache_bytes: services.max_cache_bytes.clone(),
    };
    let outcome = app::index(services, &found.repo, &found.snapshot, &request);
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
        let receipt = found.receipt().counted_by(&*services.counter);
        index_json(&receipt, &indexed)
    } else {
        Ok(index_text(&indexed))
    };
    answer(streams, rendered, code)
}
