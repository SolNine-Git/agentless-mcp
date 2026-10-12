//! One gate call's shared state: the record the steps fill, the deadline, and the one settle that
//! the decision and a SIGTERM race for.

use std::ffi::OsString;
use std::mem;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::Output;
use crate::record::{self, Outcome, Record};
use crate::verdict::Decision;

const DEADLINE: Duration = Duration::from_secs(10);
const LOG_RESERVE: Duration = Duration::from_millis(500);
const TERMINATED: &str = "terminated";

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn wall(&self) -> SystemTime;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

pub(crate) enum Phase {
    Running(Box<Record>),
    Settled,
}

pub(crate) struct Call {
    phase: Mutex<Phase>,
    clock: Box<dyn Clock>,
    start: Instant,
    log: Option<OsString>,
}

impl Call {
    pub(crate) fn new(clock: Box<dyn Clock>, log: Option<OsString>) -> Self {
        let start = clock.now();
        let record = Record::blank(record::seconds(clock.wall()));
        Self {
            phase: Mutex::new(Phase::Running(Box::new(record))),
            clock,
            start,
            log,
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn note(&self, write: impl FnOnce(&mut Record)) {
        if let Phase::Running(record) = &mut *self.lock() {
            write(record);
        }
    }

    // The two fields the internal-error line names, as CPython's %s shows them.
    pub(crate) fn whose(&self) -> (String, String) {
        let none = || "None".to_owned();
        match &*self.lock() {
            Phase::Running(record) => (
                record.tool_use_id.clone().unwrap_or_else(none),
                record.path.clone().unwrap_or_else(none),
            ),
            Phase::Settled => (none(), none()),
        }
    }

    // The time a step may use: what is left before the deadline less the log reserve, at most
    // `cap`. None when nothing is left.
    pub(crate) fn budget(&self, cap: Option<Duration>) -> Option<Duration> {
        let left = (self.start + DEADLINE).saturating_duration_since(self.clock.now());
        let budget = left
            .checked_sub(LOG_RESERVE)
            .filter(|left| !left.is_zero())?;
        Some(cap.map_or(budget, |cap| budget.min(cap)))
    }

    pub(crate) fn settle(&self, decision: Decision) -> Output {
        let Some(record) = take(&mut self.lock()) else {
            // A SIGTERM settled the call first. Its watcher exits the process and writes no reply.
            return Output::default();
        };
        self.finish(record, decision)
    }

    // The SIGTERM path: a call still running ends degraded, with its record as far as it got.
    pub(crate) fn terminate(&self, phase: &mut Phase) -> Option<Output> {
        let mut record = take(phase)?;
        record.outcome = Some(Outcome::Degraded);
        record.reason = Some(TERMINATED.to_owned());
        Some(self.finish(record, Decision::default()))
    }

    fn finish(&self, mut record: Record, decision: Decision) -> Output {
        let elapsed = self.clock.now().saturating_duration_since(self.start);
        record.latency_ms = Some(record::milliseconds(elapsed));
        let failure = self.append(&record);
        Output {
            code: decision.code,
            stdout: decision.stdout,
            stderr: format!("{failure}{}", decision.stderr),
        }
    }

    // Appends the record when a log is set, and returns the stderr lines of a failure.
    fn append(&self, record: &Record) -> String {
        let Some(target) = self.log.as_ref().filter(|target| !target.is_empty()) else {
            return String::new();
        };
        let shown = target.to_string_lossy();
        let id = record.tool_use_id.as_deref().unwrap_or("None");
        let line = match json::to_text(record, json::JsonForm::Compact) {
            Ok(text) => format!("{text}\n"),
            Err(error) => {
                return format!(
                    "erosion gate cannot append to {shown}: tool_use_id={id}\n{error}\n"
                );
            }
        };
        match io::append_line(Path::new(target), line.as_bytes()) {
            Ok(()) => String::new(),
            Err(io::AppendError::Io(error)) => {
                let repr = io::os_error_class_text(&error);
                format!("erosion gate cannot append to {shown}: tool_use_id={id}\n{repr}\n")
            }
            Err(io::AppendError::Short { written, expected }) => format!(
                "erosion gate wrote {written} of {expected} bytes to {shown}: tool_use_id={id}\n"
            ),
        }
    }
}

fn take(phase: &mut Phase) -> Option<Record> {
    match mem::replace(phase, Phase::Settled) {
        Phase::Running(record) => Some(*record),
        Phase::Settled => None,
    }
}

#[cfg(test)]
mod tests;
