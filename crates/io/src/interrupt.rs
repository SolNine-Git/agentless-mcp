//! The SIGINT watch: while a hold is active, SIGINT is recorded and the process goes on, so a test
//! run can kill its process group and a repository write can finish. With no hold active, SIGINT
//! ends the process at once, as it does without the watch.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use signal_hook::consts::SIGINT;

struct Watch {
    // Set by the first SIGINT after the registration, and never cleared: the process is ending.
    interrupted: Arc<AtomicBool>,
    // True while no hold is active; the default action of SIGINT runs only then.
    idle: Arc<AtomicBool>,
    holds: Mutex<usize>,
}

static WATCH: Mutex<Option<Arc<Watch>>> = Mutex::new(None);

// One active hold. The drop releases it.
pub(crate) struct Hold {
    watch: Arc<Watch>,
}

// Registers the watch on first use, then counts one more active hold.
pub(crate) fn hold() -> io::Result<Hold> {
    let watch = watch()?;
    let mut holds = watch.holds.lock().unwrap_or_else(PoisonError::into_inner);
    *holds += 1;
    watch.idle.store(false, Ordering::SeqCst);
    drop(holds);
    Ok(Hold { watch })
}

impl Hold {
    pub(crate) fn interrupted(&self) -> bool {
        self.watch.interrupted.load(Ordering::SeqCst)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut holds = self
            .watch
            .holds
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *holds -= 1;
        self.watch.idle.store(*holds == 0, Ordering::SeqCst);
    }
}

/// Returns true when SIGINT arrived while a test run or a repository write held it.
#[must_use]
pub fn interrupted() -> bool {
    let slot = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    slot.as_ref()
        .is_some_and(|watch| watch.interrupted.load(Ordering::SeqCst))
}

/// Ends the process with the default action of SIGINT, so its parent sees death by SIGINT. The
/// caller calls it after [`interrupted`] returns true and its own output is written.
pub fn end_by_interrupt() -> ! {
    // The emulation restores the default action and raises SIGINT, so it returns only on an error.
    let _failed = signal_hook::low_level::emulate_default_handler(SIGINT);
    std::process::abort()
}

fn watch() -> io::Result<Arc<Watch>> {
    let mut slot = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(watch) = slot.as_ref() {
        return Ok(Arc::clone(watch));
    }
    let watch = Arc::new(Watch {
        interrupted: Arc::new(AtomicBool::new(false)),
        idle: Arc::new(AtomicBool::new(true)),
        holds: Mutex::new(0),
    });
    // The record runs first, so a SIGINT during a hold is never lost to the default action.
    signal_hook::flag::register(SIGINT, Arc::clone(&watch.interrupted))?;
    signal_hook::flag::register_conditional_default(SIGINT, Arc::clone(&watch.idle))?;
    *slot = Some(Arc::clone(&watch));
    Ok(watch)
}
