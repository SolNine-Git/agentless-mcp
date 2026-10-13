//! What each file's own import statements bind, resolved to repository files once per call.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use facts::Import;

use crate::imports::{resolve_import_target, resolve_imported_submodule};
use crate::paths::PathIndex;
use crate::scan::FileFacts;

type Bindings<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

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
        let per_file = files.iter().map(|file| bind_file(file, &paths)).collect();
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

fn bind_file<'a>(file: &'a FileFacts, paths: &PathIndex<'a>) -> ImportScope<'a> {
    let mut scope = ImportScope::default();
    for (position, import) in file.facts.imports.iter().enumerate() {
        scope.bind_statement(&file.path, position, import, paths);
    }
    scope
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
