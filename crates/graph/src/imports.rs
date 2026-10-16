//! The one owner of "module string plus importing file becomes a repository path".

use facts::Import;

use crate::paths::{PathIndex, PosixPath, normalized};

/// The repository paths that import statements resolve against.
#[derive(Debug)]
pub struct ImportTargets<'a> {
    paths: PathIndex<'a>,
}

impl<'a> ImportTargets<'a> {
    /// Index `paths` for import resolution.
    #[must_use]
    pub fn build(paths: impl IntoIterator<Item = &'a str>) -> ImportTargets<'a> {
        ImportTargets {
            paths: PathIndex::build(paths),
        }
    }

    /// Return the path that `import`, written in the file at `importer`, names, or `None`.
    #[must_use]
    pub fn resolve(&self, importer: &str, import: &Import) -> Option<&'a str> {
        resolve_import_target(importer, &import.module, import.relative_level, &self.paths)
    }
}

// Ordered so that a package's entry point loses to a module of the same name.
const MODULE_SUFFIXES: [&str; 25] = [
    ".py",
    ".ts",
    ".tsx",
    ".js",
    ".jsx",
    ".mjs",
    ".cjs",
    ".go",
    ".rb",
    ".rs",
    ".java",
    ".lua",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".sh",
    ".php",
    ".kt",
    ".swift",
    "/__init__.py",
    "/index.ts",
    "/index.tsx",
    "/index.js",
    "",
];

// Unresolved is a normal outcome; the shortest tail match wins, so walk order never decides.
pub(crate) fn resolve_import_target<'a>(
    importer: &str,
    module: &str,
    relative_level: u32,
    paths: &PathIndex<'a>,
) -> Option<&'a str> {
    let module = text::strip(module);
    let relative = relative_level > 0 || module.starts_with('.');
    if module.is_empty() && !relative {
        return None;
    }
    let directory = PosixPath::parse(importer).parent();
    if let Some(found) =
        first_candidate(&candidate_bases(module, relative_level, &directory), paths)
    {
        return Some(found);
    }
    // A relative module is written against its own directory, so no other tail may answer it.
    if relative {
        return None;
    }
    paths.suffix_match(module)
}

// `from package import name`, when `name` is itself a module.
pub(crate) fn resolve_imported_submodule<'a>(
    importer: &str,
    module: &str,
    relative_level: u32,
    name: &str,
    paths: &PathIndex<'a>,
) -> Option<&'a str> {
    let dotted = if module.is_empty() {
        name.to_owned()
    } else {
        format!("{module}.{name}")
    };
    resolve_import_target(importer, &dotted, relative_level, paths)
}

// Every candidate starts with its base, so a base that no path starts with skips its suffixes.
fn first_candidate<'a>(bases: &[String], paths: &PathIndex<'a>) -> Option<&'a str> {
    bases.iter().find_map(|base| {
        if !paths.has_prefix(base) {
            return None;
        }
        MODULE_SUFFIXES.iter().find_map(|suffix| {
            let candidate = if base.is_empty() {
                suffix.strip_prefix('/').unwrap_or(suffix).to_owned()
            } else {
                format!("{base}{suffix}")
            };
            paths.get(&candidate)
        })
    })
}

fn candidate_bases(module: &str, relative_level: u32, directory: &PosixPath<'_>) -> Vec<String> {
    if relative_level > 0 {
        return level_base(module, relative_level, directory)
            .into_iter()
            .collect();
    }
    if module.starts_with('.') {
        return vec![normalized(&directory.join(module))];
    }
    // Every other module takes the file-name branch: the suffix test it ports also matches the
    // empty suffix, so the dotted form is never tried.
    vec![normalized(&directory.join(module)), module.to_owned()]
}

// A level that walks above the repository root names no file here.
fn level_base(module: &str, relative_level: u32, directory: &PosixPath<'_>) -> Option<String> {
    let steps = usize::try_from(relative_level - 1).ok()?;
    if steps > directory.part_count() {
        return None;
    }
    let base = (0..steps).fold(directory.clone(), |base, _| base.parent());
    let tail = module.replace('.', "/");
    let path = if tail.is_empty() {
        base.render()
    } else {
        base.join(&tail)
    };
    Some(normalized(&path))
}
