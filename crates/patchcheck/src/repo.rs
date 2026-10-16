//! The pre-patch repository that the checks compare a patch against.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use graph::{FileFacts, Resolver};
use syntax::Unparsable;

use crate::duplicates::{Bodies, existing_bodies};
use crate::manifest::DeclaredDependencies;

/// The scanned files of the resolver, the texts read for some of them, and the declared
/// dependencies. The files are the resolver's own scan, so the two cannot disagree; the body
/// index of `near_duplicates` is built on first use and kept for every later patch.
pub struct RepoFacts<'a> {
    resolver: &'a Resolver<'a>,
    files: BTreeMap<&'a str, &'a FileFacts>,
    texts: &'a BTreeMap<String, String>,
    dependencies: DeclaredDependencies,
    bodies: OnceLock<Result<Bodies<'a>, Unparsable>>,
}

impl<'a> RepoFacts<'a> {
    /// Return the facts of the resolver's scan with `texts` by path and `dependencies`.
    #[must_use]
    pub fn new(
        resolver: &'a Resolver<'a>,
        texts: &'a BTreeMap<String, String>,
        dependencies: DeclaredDependencies,
    ) -> RepoFacts<'a> {
        let files = resolver
            .index()
            .files()
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        RepoFacts {
            resolver,
            files,
            texts,
            dependencies,
            bodies: OnceLock::new(),
        }
    }

    pub(crate) fn resolver(&self) -> &'a Resolver<'a> {
        self.resolver
    }

    pub(crate) fn dependencies(&self) -> &DeclaredDependencies {
        &self.dependencies
    }

    // The scanned files in path order; a repeated path keeps its last file.
    pub(crate) fn files(&self) -> impl Iterator<Item = &'a FileFacts> + '_ {
        self.files.values().copied()
    }

    pub(crate) fn file_count(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn file(&self, path: &str) -> Option<&'a FileFacts> {
        self.files.get(path).copied()
    }

    // Every scanned file as the resolver holds them, for a graph built over the same slice.
    pub(crate) fn scan(&self) -> &'a [FileFacts] {
        self.resolver.index().files()
    }

    pub(crate) fn text(&self, path: &str) -> Option<&'a str> {
        self.texts.get(path).map(String::as_str)
    }

    pub(crate) fn bodies(&self) -> Result<&Bodies<'a>, Unparsable> {
        self.bodies
            .get_or_init(|| existing_bodies(self))
            .as_ref()
            .map_err(Clone::clone)
    }
}
