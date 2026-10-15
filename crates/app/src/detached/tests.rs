use std::sync::mpsc::{self, Sender};
use std::thread::{self, Builder, ThreadId};

use super::dropped_on;

// No address space holds a stack this large, so the OS refuses the thread.
const REFUSED_STACK: usize = 1 << 60;

// Sends the id of the thread that drops it.
struct Recorder(Sender<ThreadId>);

impl Drop for Recorder {
    fn drop(&mut self) {
        self.0
            .send(thread::current().id())
            .expect("the test holds the receiver");
    }
}

struct PanicsOnDrop;

impl Drop for PanicsOnDrop {
    fn drop(&mut self) {
        panic!("the drop broke");
    }
}

#[test]
fn a_started_helper_drops_the_value_off_the_calling_thread() {
    let (sender, dropped_by) = mpsc::channel();

    let helper = dropped_on(Builder::new(), Recorder(sender)).expect("the OS starts the helper");
    let helper_id = helper.thread().id();
    helper.join().expect("the drop ends");

    assert_eq!(dropped_by.try_recv(), Ok(helper_id));
    assert_ne!(helper_id, thread::current().id());
}

#[test]
fn a_refused_thread_drops_the_value_on_the_calling_thread_before_the_return() {
    let (sender, dropped_by) = mpsc::channel();
    let refused = Builder::new().stack_size(REFUSED_STACK);

    let helper = dropped_on(refused, Recorder(sender));

    assert!(helper.is_none());
    assert_eq!(dropped_by.try_recv(), Ok(thread::current().id()));
}

#[test]
fn a_panic_in_the_drop_stays_on_the_helper_thread() {
    let helper = dropped_on(Builder::new(), PanicsOnDrop).expect("the OS starts the helper");

    let payload = helper.join().expect_err("the drop panics on the helper");

    assert_eq!(payload.downcast_ref::<&str>(), Some(&"the drop broke"));
}
