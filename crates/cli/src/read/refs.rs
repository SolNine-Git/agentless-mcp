//! The step of `refs`: the fan-in of one target, or its shared callers.

use app::{BindingStatus, Reading, RefsRequest, Services};

use super::{Answering, ReadArgs, emit_then, open_reading, utf8_path};
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK};

// A target that binds to nothing is answered, and exits 1. A bound target with no reference
// exits 0.
pub(super) fn refs(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let target = args.text("target").expect("the parser requires the target");
    let (target, flags) = match utf8_path(target, "NAME_OR_STABLE_ID", streams) {
        Ok(target) => (target, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = RefsRequest {
        target,
        limit: args.int("limit"),
        shared_callers: args.flag("shared_callers"),
    };
    let outcome = app::refs(services, &Reading::of(&session), &request);
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    emit_then(outcome, &answering, streams, |result| {
        let unbound = result.binding.status == BindingStatus::NotFound;
        (String::new(), if unbound { EXIT_DOMAIN } else { EXIT_OK })
    })
}
