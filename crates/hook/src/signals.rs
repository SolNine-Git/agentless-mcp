//! The SIGTERM watch: a call still running when the signal arrives logs "terminated" and exits 0.

use std::io::Write;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use signal_hook::consts::SIGTERM;
use signal_hook::iterator::{Handle, Signals};

use crate::call::Call;

pub(crate) struct Watch {
    handle: Handle,
    thread: JoinHandle<()>,
}

// From here on SIGTERM no longer kills the process: it settles the call instead.
pub(crate) fn watch(call: Arc<Call>) -> std::io::Result<Watch> {
    let mut signals = Signals::new([SIGTERM])?;
    let handle = signals.handle();
    let thread = thread::Builder::new()
        .name("erosion-sigterm".to_owned())
        .spawn(move || {
            for _ in signals.forever() {
                terminate(&call);
            }
        })?;
    Ok(Watch { handle, thread })
}

impl Watch {
    // After the decision SIGTERM is ignored: the registry keeps its handler with no action.
    pub(crate) fn stop(self) {
        self.handle.close();
        if let Err(payload) = self.thread.join() {
            std::panic::resume_unwind(payload);
        }
    }
}

// The lock stays held through the exit, so the decision can never write after this.
fn terminate(call: &Call) {
    let mut phase = call.lock();
    let Some(output) = call.terminate(&mut phase) else {
        return;
    };
    // The caller can hold the stderr lock while the gate runs, so only a failed log append
    // takes it. stderr is the last sink: a failed write has nowhere to go, and the exit is 0.
    if !output.stderr.is_empty() {
        let _unwritten = std::io::stderr().lock().write_all(output.stderr.as_bytes());
    }
    std::process::exit(0);
}
