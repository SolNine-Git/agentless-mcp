//! The error an operation returns.

use kernel::Stopped;

const CANCELLED: &str = "the client cancelled this call, so it stopped at its next checkpoint";

/// Why an operation gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A domain refusal or failure, with its kind and message.
    #[error(transparent)]
    Kind(#[from] kernel::Error),
    /// A pair check failed on input it cannot read: a defect, never a property of the change.
    #[error(transparent)]
    Check(#[from] checks::Error),
    /// A row count of the tag cache failed after the cache opened.
    #[error(transparent)]
    Store(#[from] store::SqlError),
}

impl From<Stopped> for Error {
    fn from(stopped: Stopped) -> Error {
        kernel::Error::CallStopped(stopped_text(stopped)).into()
    }
}

fn stopped_text(stopped: Stopped) -> String {
    let Stopped::Spent {
        elapsed,
        limit,
        grace,
        credited,
    } = stopped
    else {
        return CANCELLED.to_owned();
    };
    format!(
        "this call stopped after {} s. The server allows {} s of work per call, plus up to {} s \
         for parsing files the tag cache does not hold, and this call had {} s of that grace. \
         Narrow the request, or run `agentless-mcp index` on the repository so that later calls \
         read a warm cache. The server flags --call-limit and --recache-grace set the two bounds.",
        json::fixed(elapsed, 0),
        json::general(limit),
        json::general(grace),
        json::fixed(credited, 0),
    )
}

#[cfg(test)]
mod tests;
