//! The drain before a stop: every permit of the gate back, so each call that arrived before the
//! stop has made its answer. The graceful close after the drain writes the answer to its stream.

use std::pin::pin;

use tokio::sync::Semaphore;

use crate::service::first;

// True when every permit came back before `expired` finished. The gate is first in, first out, so
// a call that waits for a permit gets one before the drain does.
pub(crate) async fn drained(
    gate: &Semaphore,
    permits: usize,
    expired: impl Future<Output = ()>,
) -> bool {
    let permits = u32::try_from(permits).unwrap_or(u32::MAX);
    let expired = pin!(expired);
    let taken = first(gate.acquire_many(permits), expired).await;
    taken.is_some_and(|permits| permits.is_ok())
}
