//! The step of `cycles`: the module-level import cycles.

use app::{CyclesRequest, Reading, Services};

use super::{ReadArgs, emit, open_reading};
use crate::Streams;

pub(super) fn cycles(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = CyclesRequest {
        limit: args.int("limit"),
    };
    let outcome = app::cycles(services, &Reading::of(&session), &request);
    emit(outcome, &session, services, &flags, streams)
}
