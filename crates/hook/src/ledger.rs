//! The ledger: which files a context holds a card for. A context is the main conversation or one
//! subagent, and an entry's key holds the file's content hash, so an edit makes the card stale.
//! Layout: `<sha256(session)>/<"main" or sha256(agent)>/<sha256(path, NUL, sha256(content))>`,
//! one empty file per entry, and the pending subagent briefs in `<sha256(session)>/briefs/`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use app::EnvConfig;

use crate::run::Context;
use crate::state::{self, Claim};
use crate::text::sha256_hex;

const LEDGER_DIR: &str = "agentless_ledger";
const MAIN: &str = "main";
const BRIEFS: &str = "briefs";
// The entries of one context. Past the cap a context gets no new card.
pub(crate) const CAP: usize = 2_000;

// The outcome of one entry's claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claimed {
    New,
    Held,
    Full,
    Unavailable,
}

pub(crate) fn base(env: &EnvConfig) -> PathBuf {
    state::temporary_dir(env, LEDGER_DIR)
}

fn session_dir(base: &Path, session: &str) -> PathBuf {
    base.join(sha256_hex(session.as_bytes()))
}

fn context_dir(base: &Path, context: &Context) -> PathBuf {
    let agent = context.agent.as_deref();
    let name = agent.map_or_else(|| MAIN.to_owned(), |agent| sha256_hex(agent.as_bytes()));
    session_dir(base, &context.session).join(name)
}

pub(crate) fn briefs_dir(base: &Path, session: &str) -> PathBuf {
    session_dir(base, session).join(BRIEFS)
}

// The entry's name: the digest of the canonical path, a NUL and the hex digest of the content.
fn entry_name(canonical: &Path, bytes: &[u8]) -> String {
    let mut key = canonical.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(sha256_hex(bytes).as_bytes());
    sha256_hex(&key)
}

pub(crate) fn claim(base: &Path, context: &Context, canonical: &Path, bytes: &[u8]) -> Claimed {
    if !state::private_dir(base) {
        return Claimed::Unavailable;
    }
    let dir = context_dir(base, context);
    if io::create_missing_owner_dir(&dir).is_err() {
        return Claimed::Unavailable;
    }
    let entry = dir.join(entry_name(canonical, bytes));
    if let Some(full) = held_or_full(&dir, &entry) {
        return full;
    }
    match state::claim(&entry) {
        Claim::New => Claimed::New,
        Claim::Held => Claimed::Held,
        Claim::Unavailable => Claimed::Unavailable,
    }
}

// A held entry stays held at the cap; only a new one counts against it.
fn held_or_full(dir: &Path, entry: &Path) -> Option<Claimed> {
    if state::is_plain_file(entry) {
        return Some(Claimed::Held);
    }
    match fs::read_dir(dir) {
        Ok(entries) => (entries.take(CAP).count() >= CAP).then_some(Claimed::Full),
        Err(_) => Some(Claimed::Unavailable),
    }
}

// Why a reset removed nothing: storage that is not private, or the removal's error.
#[derive(Debug)]
pub(crate) enum ResetFailed {
    Untrusted,
    Removal(std::io::Error),
}

// Removes the main context's entries: Ok(true) when there were any.
pub(crate) fn reset(base: &Path, session: &str) -> Result<bool, ResetFailed> {
    if !state::private_dir(base) {
        return Err(ResetFailed::Untrusted);
    }
    let main = session_dir(base, session).join(MAIN);
    match io::remove_tree(&main) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ResetFailed::Removal(error)),
    }
}
