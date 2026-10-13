//! The step of `refs`: the fan-in of one target, or its shared callers.

use app::{Reading, RefsRequest, Services};

use super::{ReadArgs, emit, open_reading, utf8_path};
use crate::Streams;

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
    emit(outcome, &session, services, &flags, streams)
}
