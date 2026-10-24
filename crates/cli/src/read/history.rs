//! The step of `history`: the commits that touched one symbol's lines.

use app::{HistoryRequest, Reading, Services};

use super::{ReadArgs, emit, open_reading, utf8_path};
use crate::Streams;

pub(super) fn history(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let target = args.text("target").expect("the parser requires STABLE_ID");
    let (target, flags) = match utf8_path(target, "STABLE_ID", streams) {
        Ok(target) => (target, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = HistoryRequest {
        target,
        limit: args.int("limit"),
        budget: args.int("budget"),
    };
    let outcome = app::history(services, &Reading::of(&session), &request);
    emit(outcome, &session, services, &flags, streams)
}
