use std::future::{pending, ready};
use std::sync::Arc;

use tokio::sync::{Semaphore, oneshot};

use crate::drain::drained;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
}

#[test]
fn with_no_call_in_flight_the_drain_ends_at_once() {
    let gate = Semaphore::new(3);
    assert!(runtime().block_on(drained(&gate, 3, pending())));
}

// The drain cannot end while a call holds its permit; the call's end lets it end.
#[test]
fn a_call_in_flight_holds_the_drain_until_it_ends() {
    runtime().block_on(async {
        let gate = Arc::new(Semaphore::new(2));
        let held = Arc::clone(&gate).acquire_owned().await.expect("a permit");
        let draining = Arc::clone(&gate);
        let drain = tokio::spawn(async move { drained(&draining, 2, pending()).await });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(!drain.is_finished());
        drop(held);
        assert!(drain.await.expect("a drain task"));
    });
}

#[test]
fn the_bound_ends_the_drain_before_a_stuck_call() {
    runtime().block_on(async {
        let gate = Arc::new(Semaphore::new(2));
        let _held = Arc::clone(&gate).acquire_owned().await.expect("a permit");
        assert!(!drained(&gate, 2, ready(())).await);
        let (expire, expired) = oneshot::channel::<()>();
        drop(expire);
        assert!(!drained(&gate, 2, async { drop(expired.await) }).await);
    });
}
