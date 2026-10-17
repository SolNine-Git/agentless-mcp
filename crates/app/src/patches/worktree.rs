//! The life of one scratch worktree of a call: the cache root, the creation, the work in it, and
//! the release. A worktree apply and every worktree of a validate run share it.
//!
//! The worktree holds SIGINT from before its creation until after its release, so a SIGINT never
//! leaves it registered in the repository. The call's SIGINT check runs before the creation,
//! before the work and after the release, and a SIGINT it sees wins over every other outcome.

use std::path::Path;

use git::Worktree;

use crate::{Error, Noted, Services};

// The repository and the commit that the worktrees of one call check out, and how the call sees
// and reports a SIGINT.
pub(crate) struct Scratch<'a> {
    pub(crate) services: &'a Services,
    pub(crate) root: &'a Path,
    pub(crate) commit: &'a str,
    pub(crate) interrupted: &'a dyn Fn() -> bool,
    pub(crate) stopped: fn() -> Error,
}

impl Scratch<'_> {
    // A worktree for `work`, released after it. The cache-home lines that `ignored` returns come
    // first, then the creation's cleanup notes, or the work's lines and then the release notes.
    pub(crate) fn run<T>(
        &self,
        ignored: impl FnOnce(&io::CacheRoot) -> Vec<String>,
        work: impl FnOnce(&Worktree) -> Noted<Result<T, Error>>,
    ) -> Noted<Result<T, Error>> {
        if (self.interrupted)() {
            return Noted::new(Err((self.stopped)()));
        }
        let cache = match &self.services.cache_root {
            Ok(cache) => cache,
            Err(home) => {
                let error = kernel::Error::OperationFailed(home.to_string());
                return Noted::new(Err(error.into()));
            }
        };
        let mut diagnostics = ignored(cache);
        let created = self
            .services
            .git
            .worktree(self.root, self.commit, &cache.path);
        let tree = match created {
            Ok(tree) => tree,
            Err(failure) => {
                diagnostics.extend(failure.cleanup.iter().map(ToString::to_string));
                let value = self.unless_stopped(Err(kernel::Error::from(failure.error).into()));
                return Noted { value, diagnostics };
            }
        };
        let worked = if (self.interrupted)() {
            Noted::new(Err((self.stopped)()))
        } else {
            work(&tree)
        };
        diagnostics.extend(worked.diagnostics);
        diagnostics.extend(tree.release().iter().map(ToString::to_string));
        Noted {
            value: self.unless_stopped(worked.value),
            diagnostics,
        }
    }

    fn unless_stopped<T>(&self, value: Result<T, Error>) -> Result<T, Error> {
        if (self.interrupted)() {
            return Err((self.stopped)());
        }
        value
    }
}

// The lines of an ignored cache home, for a call that prints them before each worktree.
pub(crate) fn every_ignored(cache: &io::CacheRoot) -> Vec<String> {
    cache.ignored.iter().map(ToString::to_string).collect()
}
