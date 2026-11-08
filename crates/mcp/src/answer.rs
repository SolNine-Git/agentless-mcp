//! One call's answer: the allowlist and the selected repository, its session, the background index,
//! then the tool's work and the text of its answer under the receipt.

mod search;
mod trace;

use std::borrow::Cow;
use std::path::PathBuf;

use app::{Reading, Session};
use git::Snapshot;
use kernel::Deadline;
use render::{Receipt, View};

use crate::dispatch::{Call, Inner};
use crate::failure::Failure;
use crate::hooks::SessionNote;
use crate::roots::{Anchor, Selection, selected, unauthorised};

// What a tool reads: the services, the session, its reading, the allowed roots and the claim of
// the client session on the hook note.
pub(crate) struct Context<'a> {
    pub(crate) inner: &'a Inner,
    pub(crate) reading: Reading<'a>,
    pub(crate) session: &'a Session,
    pub(crate) allowed: &'a [PathBuf],
    pub(crate) note: &'a SessionNote,
}

impl Context<'_> {
    pub(crate) fn logged(&self, diagnostics: &[String]) {
        for line in diagnostics {
            self.inner.ports.log.line(line);
        }
    }

    // The text of `view` under the receipt, whose cache field reads as of now. The allowlist holds
    // each root once, so its length is the count of selectable roots.
    pub(crate) fn rendered(&self, view: &impl View) -> String {
        let cache = self.cache_field();
        let session = self.session;
        let snapshot = self.noted_snapshot();
        let receipt = Receipt::new(&session.repo, &snapshot, &cache)
            .counted_by(&*self.inner.services.counter)
            .among(self.allowed.len());
        render::text(&receipt, view)
    }

    // The first answer of a client session carries the hook note, when there is one, after the
    // notes of the git state.
    fn noted_snapshot(&self) -> Cow<'_, Snapshot> {
        let snapshot = &self.session.snapshot;
        let first = self.note.claim();
        let Some(hooks) = first
            .then(|| self.inner.hooks.note(&self.session.repo.root))
            .flatten()
        else {
            return Cow::Borrowed(snapshot);
        };
        let note = if snapshot.note.is_empty() {
            hooks
        } else {
            format!("{}; {hooks}", snapshot.note)
        };
        Cow::Owned(Snapshot {
            note,
            ..snapshot.clone()
        })
    }

    // While a background refresh of the database runs, the cache field names it in place of the
    // remediation, so it is read when the answer renders.
    pub(crate) fn cache_field(&self) -> String {
        let report = &self.session.cache;
        let refreshing = self.inner.ports.arming.refreshing(&report.status.path);
        match report.counts {
            Ok(_) => report.status.receipt_while(refreshing),
            Err(_) => report.receipt.clone(),
        }
    }
}

// The text of `call` against the repository it selects, or why it has none.
pub(crate) fn answer(
    inner: &Inner,
    call: &Call,
    deadline: &Deadline,
    note: &SessionNote,
) -> Result<String, Failure> {
    let allowed = inner.allowlist.current()?;
    let session = opened(inner, call, &allowed)?;
    let context = Context {
        inner,
        reading: Reading::of(&session).with_deadline(deadline),
        session: &session,
        allowed: &allowed,
        note,
    };
    match call.tool.name {
        "trace" => trace::answer(&context, &call.arguments),
        "search" => search::answer(&context, &call.arguments),
        other => Err(Failure::Unplanned(format!(
            "no step answers the tool {other}"
        ))),
    }
}

// The session of the selected repository, armed for the background index. A refresh thread that
// the system refuses fails the call, as the reference's does.
fn opened(inner: &Inner, call: &Call, allowed: &[PathBuf]) -> Result<Session, Failure> {
    let services = &*inner.services;
    let hint = inner.allowlist.hint();
    let anchor = Anchor::of(services);
    let selection = Selection {
        allowed,
        requested: call.arguments.text("repo_root"),
        launch: inner.launch.as_deref(),
        anchor: &anchor,
        hint: hint.as_deref(),
    };
    let root = selected(&selection)?;
    let repo = app::Repo::load(services, &root)?;
    // The load resolves the root again, so a symlink swapped in after the check is refused here.
    if repo.root != root {
        return Err(unauthorised(&selection));
    }
    let opening = Session::open_facts(services, repo, false);
    for line in &opening.session.diagnostics {
        inner.ports.log.line(line);
    }
    let session = opening.session.value?;
    inner
        .ports
        .arming
        .arm(&session)
        .map_err(Failure::Unplanned)?;
    Ok(session)
}
