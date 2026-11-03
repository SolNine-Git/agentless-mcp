//! What each file's own import statements bind, resolved to repository files once per call.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use facts::Import;
use syntax::Language;

use crate::imports::{resolve_import_target, resolve_imported_submodule};
use crate::paths::PathIndex;
use crate::rust_paths::RustTree;
use crate::scan::FileFacts;

type Bindings<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

// The binding passes after the first that let a path segment name a module that a Rust file
// re-exports: a re-export of a re-export takes the second.
const EXPORT_PASSES: usize = 2;

// One import statement, or one module it names, and the repository file it reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedImport<'a> {
    pub(crate) module: Cow<'a, str>,
    pub(crate) line: u32,
    pub(crate) resolved: Option<&'a str>,
    pub(crate) relative: bool,
    pub(crate) statement: usize,
}

/// What one file's import statements bring in: whole modules, module objects, named members.
#[derive(Clone, Debug, Default)]
pub struct ImportScope<'a> {
    pub(crate) wholesale: BTreeSet<&'a str>,
    pub(crate) module_bindings: Bindings<'a>,
    pub(crate) named: Bindings<'a>,
    pub(crate) members: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str)>>,
    pub(crate) statements: Vec<ResolvedImport<'a>>,
    // Each Rust module path that qualifies a name here, with what it names.
    pub(crate) qualified: BTreeMap<&'a str, Qualified<'a>>,
}

// What a Rust module path names: a module file of this repository, or a module outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Qualified<'a> {
    Module(&'a str),
    Foreign,
}

impl<'a> ImportScope<'a> {
    /// Return the local names that a `from x import member as local` binds, in name order.
    pub fn member_names(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.members.keys().copied()
    }

    /// Return `(module, line, target)` for each resolved statement of the file at `path` that is
    /// an import edge; a file importing itself is no edge.
    pub fn resolved_edges<'s>(
        &'s self,
        path: &'s str,
    ) -> impl Iterator<Item = (&'s str, u32, &'a str)> + 's {
        self.statements.iter().filter_map(move |row| {
            let target = row.resolved.filter(|target| *target != path)?;
            Some((row.module.as_ref(), row.line, target))
        })
    }

    // Only C's #include and Python's `from x import *` bring every name in unqualified.
    pub(crate) fn binds(&self, name: &str, path: &str) -> bool {
        self.wholesale.contains(path)
            || self
                .named
                .get(name)
                .is_some_and(|paths| paths.contains(path))
    }
}

/// The import scope of every scanned file.
#[derive(Clone, Debug)]
pub struct Scopes<'a> {
    files: &'a [FileFacts],
    per_file: Vec<ImportScope<'a>>,
    by_path: Vec<(&'a str, usize)>,
}

impl<'a> Scopes<'a> {
    /// Resolve every file's import statements against the scanned paths.
    #[must_use]
    pub fn build(files: &'a [FileFacts]) -> Scopes<'a> {
        let paths = PathIndex::build(files.iter().map(|file| file.path.as_str()));
        let mut rust = RustTree::build(files, &paths);
        let mut per_file: Vec<ImportScope<'a>> = files
            .iter()
            .map(|file| bind_file(file, &paths, &rust))
            .collect();
        if rust.holds_manifest() {
            for _ in 0..EXPORT_PASSES {
                rust.exports = exports_of(files, &per_file);
                rebind_rust(files, &mut per_file, &paths, &rust);
            }
        }
        let mut by_path: Vec<(&'a str, usize)> = files
            .iter()
            .enumerate()
            .map(|(position, file)| (file.path.as_str(), position))
            .collect();
        by_path.sort_unstable();
        Scopes {
            files,
            per_file,
            by_path,
        }
    }

    /// Return the scanned files the scopes were built from.
    #[must_use]
    pub fn files(&self) -> &'a [FileFacts] {
        self.files
    }

    /// Return the scope of the file at `path`.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&ImportScope<'a>> {
        self.position(path).map(|position| &self.per_file[position])
    }

    // The language of the file at `path`.
    pub(crate) fn language_of(&self, path: &str) -> Option<Language> {
        self.position(path)
            .map(|position| self.files[position].language)
    }

    // The scan position of the file at `path`; a repeated path answers with its last file.
    pub(crate) fn position(&self, path: &str) -> Option<usize> {
        let end = self.by_path.partition_point(|(held, _)| *held <= path);
        let (held, position) = *self.by_path[..end].last()?;
        (held == path).then_some(position)
    }

    /// Return each file's path with its scope, in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &ImportScope<'a>)> + '_ {
        self.by_path
            .iter()
            .map(|&(path, position)| (path, &self.per_file[position]))
    }

    /// Return the `(importer, imported)` file pairs that a resolved import connects.
    #[must_use]
    pub fn import_pairs(&self) -> BTreeSet<(&'a str, &'a str)> {
        self.files
            .iter()
            .zip(&self.per_file)
            .flat_map(|(file, scope)| {
                let path = file.path.as_str();
                scope
                    .resolved_edges(path)
                    .map(move |(_, _, target)| (path, target))
            })
            .collect()
    }

    // The scope of the file at `position` in the scan.
    pub(crate) fn at(&self, position: usize) -> &ImportScope<'a> {
        &self.per_file[position]
    }
}

struct Statement<'a> {
    path: &'a str,
    import: &'a Import,
    bound: Option<&'a str>,
    relative: bool,
    position: usize,
}

fn bind_file<'a>(
    file: &'a FileFacts,
    paths: &PathIndex<'a>,
    rust: &RustTree<'_, 'a>,
) -> ImportScope<'a> {
    let mut scope = ImportScope::default();
    if file.language != Language::Rust {
        for (position, import) in file.facts.imports.iter().enumerate() {
            scope.bind_statement(&file.path, position, import, paths);
        }
        return scope;
    }
    for (position, import) in file.facts.imports.iter().enumerate() {
        scope.bind_rust_statement(&file.path, position, import, rust);
    }
    scope.qualify(file, rust);
    scope
}

fn rebind_rust<'a>(
    files: &'a [FileFacts],
    per_file: &mut [ImportScope<'a>],
    paths: &PathIndex<'a>,
    rust: &RustTree<'_, 'a>,
) {
    let rebound = files.iter().zip(per_file);
    for (file, scope) in rebound.filter(|(file, _)| file.language == Language::Rust) {
        *scope = bind_file(file, paths, rust);
    }
}

// The module file that each Rust file binds under one local name, for the next binding pass.
fn exports_of<'a>(
    files: &'a [FileFacts],
    per_file: &[ImportScope<'a>],
) -> BTreeMap<(&'a str, &'a str), &'a str> {
    let mut exports = BTreeMap::new();
    let rust = files
        .iter()
        .zip(per_file)
        .filter(|(file, _)| file.language == Language::Rust);
    for (file, scope) in rust {
        for (local, targets) in &scope.module_bindings {
            if let (1, Some(target)) = (targets.len(), targets.first()) {
                exports.insert((file.path.as_str(), *local), *target);
            }
        }
    }
    exports
}

// The local names that the file's `use` declarations of this repository's paths bind, resolved
// or not.
fn internal_locals<'a>(
    path: &'a str,
    imports: &'a [Import],
    rust: &RustTree<'_, 'a>,
) -> BTreeSet<&'a str> {
    let mut locals = BTreeSet::new();
    for import in imports {
        let first = import.module.split("::").next().unwrap_or_default();
        for (member, local) in import.bound_names() {
            let lead = if first.is_empty() {
                member.split("::").next().unwrap_or_default()
            } else {
                first
            };
            if rust.names_repository_module(path, lead) {
                locals.insert(local);
            }
        }
        if import.names.is_empty() && rust.names_repository_module(path, first) {
            locals.insert(import.module.rsplit("::").next().unwrap_or_default());
        }
    }
    locals
}

// One `use` declaration's walks: the module tree, the importing file, and how many inline modules
// enclose the declaration.
struct Walk<'r, 'p, 'a> {
    rust: &'r RustTree<'p, 'a>,
    path: &'a str,
    inline: u32,
}

impl<'a> Walk<'_, '_, 'a> {
    fn resolve(&self, segments: &[&str]) -> Option<(&'a str, usize)> {
        self.rust.resolve(self.path, segments, self.inline)
    }

    // The module that a whole path names, when every segment names a module.
    fn whole(&self, segments: &[&str]) -> Option<&'a str> {
        self.resolve(segments)
            .filter(|(_, used)| *used == segments.len())
            .map(|(target, _)| target)
    }
}

impl<'a> ImportScope<'a> {
    // Each leaf of one `use` declaration walks the module tree; the declaration keeps one row
    // per distinct target, or one unresolved row when no leaf resolves.
    fn bind_rust_statement(
        &mut self,
        path: &'a str,
        position: usize,
        import: &'a Import,
        rust: &RustTree<'_, 'a>,
    ) {
        let walk = Walk {
            rust,
            path,
            inline: import.inline_depth,
        };
        let base: Vec<&'a str> = if import.module.is_empty() {
            Vec::new()
        } else {
            import.module.split("::").collect()
        };
        let mut targets: Vec<(String, &'a str)> = Vec::new();
        if import.names.is_empty()
            && let Some(target) = walk.whole(&base)
        {
            if import.binds_all {
                self.wholesale.insert(target);
            } else {
                self.bind_module(base.last().copied().unwrap_or_default(), target);
            }
            targets.push((import.module.clone(), target));
        }
        for (member, local) in import.bound_names() {
            let mut segments = base.clone();
            segments.extend(member.split("::"));
            if let Some(target) = self.bind_leaf(&walk, &mut segments, local) {
                targets.push((segments.join("::"), target));
            }
        }
        self.push_rust_rows(import, position, targets);
    }

    // A glob binds the module wholesale, `self` binds the module, any other leaf an item; the
    // segments end as the module path the row names.
    fn bind_leaf(
        &mut self,
        walk: &Walk<'_, '_, 'a>,
        segments: &mut Vec<&'a str>,
        local: &'a str,
    ) -> Option<&'a str> {
        let last = segments.pop().unwrap_or_default();
        match last {
            "*" => walk.whole(segments).inspect(|target| {
                self.wholesale.insert(target);
            }),
            "self" => walk.whole(segments).inspect(|target| {
                let local = if local == "self" {
                    segments.last().copied().unwrap_or_default()
                } else {
                    local
                };
                self.bind_module(local, target);
            }),
            _ => {
                segments.push(last);
                self.bind_item(walk, segments, local)
            }
        }
    }

    fn push_rust_rows(
        &mut self,
        import: &'a Import,
        position: usize,
        targets: Vec<(String, &'a str)>,
    ) {
        let mut seen: Vec<&'a str> = Vec::new();
        for (module, target) in targets {
            if seen.contains(&target) {
                continue;
            }
            seen.push(target);
            self.statements.push(ResolvedImport {
                module: Cow::Owned(module),
                line: import.line,
                resolved: Some(target),
                relative: import.is_relative,
                statement: position,
            });
        }
        if seen.is_empty() {
            self.statements.push(ResolvedImport {
                module: Cow::Borrowed(&import.module),
                line: import.line,
                resolved: None,
                relative: import.is_relative,
                statement: position,
            });
        }
    }

    // A path whose every segment names a module binds that module; one whose last segment does
    // not binds that item of the module before it; a longer item path binds nothing.
    fn bind_item(
        &mut self,
        walk: &Walk<'_, '_, 'a>,
        segments: &[&'a str],
        local: &'a str,
    ) -> Option<&'a str> {
        let (target, used) = walk.resolve(segments)?;
        if used == segments.len() {
            self.bind_module(local, target);
        } else if used + 1 == segments.len() && !local.is_empty() && local != "_" {
            let member = segments[used];
            self.named.entry(local).or_default().insert(target);
            self.members
                .entry(local)
                .or_default()
                .insert((target, member));
        }
        Some(target)
    }

    fn bind_module(&mut self, local: &'a str, target: &'a str) {
        if local.is_empty() || local == "_" {
            return;
        }
        self.named.entry(local).or_default().insert(target);
        self.module_bindings
            .entry(local)
            .or_default()
            .insert(target);
    }

    // The module each qualifier of the file's references names; a qualifier that starts with a
    // repository name the file's `use` leaves unbound stays out, so its names fall back.
    fn qualify(&mut self, file: &'a FileFacts, rust: &RustTree<'_, 'a>) {
        let internal = internal_locals(&file.path, &file.facts.imports, rust);
        for reference in &file.facts.refs {
            let qualifier = reference.qualifier.as_str();
            if qualifier.is_empty() || self.qualified.contains_key(qualifier) {
                continue;
            }
            let first = qualifier.split("::").next().unwrap_or_default();
            if internal.contains(first) && !self.module_bindings.contains_key(first) {
                continue;
            }
            if let Some(found) = self.qualified_module(&file.path, qualifier, rust) {
                self.qualified.insert(qualifier, found);
            }
        }
    }

    // What a qualifier names: `None` when its walk stops inside this repository, or when a lone
    // local name binds several modules.
    fn qualified_module(
        &self,
        path: &'a str,
        qualifier: &'a str,
        rust: &RustTree<'_, 'a>,
    ) -> Option<Qualified<'a>> {
        let segments: Vec<&str> = qualifier.split("::").collect();
        let first = segments[0];
        if segments.len() == 1
            && let Some(targets) = self.module_bindings.get(first)
        {
            let only = targets.first().copied().filter(|_| targets.len() == 1);
            return only.map(Qualified::Module);
        }
        if let Some((module, used)) = rust.resolve(path, &segments, 0) {
            return (used == segments.len()).then_some(Qualified::Module(module));
        }
        let known = self.module_bindings.contains_key(first)
            || self.named.contains_key(first)
            || rust.names_repository_module(path, first);
        (!known).then_some(Qualified::Foreign)
    }
}

impl<'a> ImportScope<'a> {
    fn bind_statement(
        &mut self,
        path: &'a str,
        position: usize,
        import: &'a Import,
        paths: &PathIndex<'a>,
    ) {
        let target = resolve_import_target(path, &import.module, import.relative_level, paths);
        let relative = import.is_relative || import.relative_level > 0;
        self.statements.push(ResolvedImport {
            module: Cow::Borrowed(&import.module),
            line: import.line,
            resolved: target,
            relative,
            statement: position,
        });
        let bound = target.filter(|target| *target != path);
        if let Some(bound) = bound.filter(|_| import.binds_all) {
            self.wholesale.insert(bound);
        }
        if import.names.is_empty() {
            self.bind_module_object(path, import, paths);
            return;
        }
        let statement = Statement {
            path,
            import,
            bound,
            relative,
            position,
        };
        for (member, local) in import.bound_names() {
            self.bind_member(&statement, member, local, paths);
        }
    }

    // `from pkg import mod` binds a module object; any other member binds a name of the target.
    fn bind_member(
        &mut self,
        statement: &Statement<'a>,
        member: &'a str,
        local: &'a str,
        paths: &PathIndex<'a>,
    ) {
        let import = statement.import;
        let level = import.relative_level;
        let submodule =
            resolve_imported_submodule(statement.path, &import.module, level, member, paths)
                .filter(|submodule| *submodule != statement.path);
        if let Some(submodule) = submodule {
            self.named.entry(local).or_default().insert(submodule);
            self.module_bindings
                .entry(local)
                .or_default()
                .insert(submodule);
            self.statements.push(ResolvedImport {
                module: Cow::Owned(dotted(&import.module, member)),
                line: import.line,
                resolved: Some(submodule),
                relative: statement.relative,
                statement: statement.position,
            });
        } else if let Some(bound) = statement.bound {
            self.named.entry(local).or_default().insert(bound);
            self.members
                .entry(local)
                .or_default()
                .insert((bound, member));
        }
    }

    // Unaliased, `import a.b` binds `a`, the package; aliased, it binds the submodule.
    fn bind_module_object(&mut self, path: &'a str, import: &'a Import, paths: &PathIndex<'a>) {
        let referenced = if import.alias.is_empty() {
            import.module.split('.').next().unwrap_or_default()
        } else {
            import.module.as_str()
        };
        let binding = if import.alias.is_empty() {
            referenced
        } else {
            import.alias.as_str()
        };
        if binding.is_empty() {
            return;
        }
        let target = resolve_import_target(path, referenced, import.relative_level, paths);
        if let Some(target) = target.filter(|target| *target != path) {
            self.module_bindings
                .entry(binding)
                .or_default()
                .insert(target);
        }
    }
}

fn dotted(module: &str, member: &str) -> String {
    if module.is_empty() {
        member.to_owned()
    } else {
        format!("{module}.{member}")
    }
}
