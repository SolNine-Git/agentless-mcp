//! Value types every agentless crate shares: errors, limits, bounded rows, digests, spans, and ids,
//! and the clock port and the call deadline.

mod bounded;
mod clock;
mod deadline;
mod error;
mod id;
mod key;
mod limit;
mod span;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use bounded::Bounded;
pub use clock::{Clock, SystemClock};
pub use deadline::{Deadline, Stopped, checkpoint, recaching, walk_budget};
pub use error::Error;
pub use id::{FileId, NameId, NodeId};
pub use key::{Key, KeyParseError};
pub use limit::Limit;
pub use span::Span;
