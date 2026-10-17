//! One call's answer: the allowlist and the selected repository, its session, the background index,
//! then the operation and the text of its view under the receipt.

mod orient;
mod read;
mod shared;
mod symbols;

use std::path::PathBuf;

use app::{Noted, Reading, Session};
use kernel::Deadline;
use render::{Receipt, View};

use crate::dispatch::{Call, Inner};
use crate::failure::Failure;
use crate::roots::{Anchor, Selection, selected};

// What an operation reads: the services, the session, the receipt and the logged warnings.
pub(crate) struct Context<'a> {
    pub(crate) inner: &'a Inner,
    pub(crate) reading: Reading<'a>,
    pub(crate) receipt: Receipt<'a>,
    pub(crate) allowed: &'a [PathBuf],
}

impl Context<'_> {
    // The text of an operation's view, after its warnings go to the log.
    pub(crate) fn viewed<V: View>(
        &self,
        outcome: Noted<Result<V, app::Error>>,
    ) -> Result<String, Failure> {
        self.logged(&outcome.diagnostics);
        let view = outcome.value?;
        Ok(render::text(&self.receipt, &view))
    }

    pub(crate) fn logged(&self, diagnostics: &[String]) {
        for line in diagnostics {
            self.inner.ports.log.line(line);
        }
    }
}

// The text of `call` against the repository it selects, or why it has none.
pub(crate) fn answer(
    inner: &Inner,
    call: &Call,
    deadline: Option<&Deadline>,
) -> Result<String, Failure> {
    let allowed = inner.allowlist.current()?;
    let session = opened(inner, call, &allowed)?;
    let services = &*inner.services;
    let reading = Reading::of(&session);
    let context = Context {
        inner,
        reading: deadline.map_or(reading, |deadline| reading.with_deadline(deadline)),
        receipt: Receipt::of(&session).counted_by(&*services.counter),
        allowed: &allowed,
    };
    match call.tool.name {
        "capabilities" => shared::capabilities(&context, &session),
        "find_referencing_symbols" => shared::refs(&context, &call.arguments),
        "history" => shared::history(&context, &call.arguments),
        "orient" => orient::answer(&context, call),
        "symbols" => symbols::answer(&context, call),
        "read" => read::answer(&context, call),
        other => Err(Failure::Unplanned(format!(
            "no operation answers the tool {other}"
        ))),
    }
}

// The session of the selected repository, armed for the background index unless the call bypasses
// the cache.
fn opened(inner: &Inner, call: &Call, allowed: &[PathBuf]) -> Result<Session, Failure> {
    let services = &*inner.services;
    let hint = inner.allowlist.hint();
    let root = selected(&Selection {
        allowed,
        requested: call.arguments.text("repo_root"),
        launch: inner.launch.as_deref(),
        anchor: &Anchor::of(services),
        hint: hint.as_deref(),
    })?;
    let repo = app::Repo::load(services, &root)?;
    let no_cache = call.arguments.flag("no_cache");
    let opening = Session::open_facts(services, repo, no_cache);
    for line in &opening.session.diagnostics {
        inner.ports.log.line(line);
    }
    let session = opening.session.value?;
    if !no_cache {
        inner.ports.arming.arm(&session);
    }
    Ok(session)
}
