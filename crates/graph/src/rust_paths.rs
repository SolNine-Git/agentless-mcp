//! Rust module paths over the scanned files: the workspace crates come from the scanned Cargo
//! manifests, and the module tree comes from the file layout.

use std::collections::BTreeMap;

use syntax::Language;

use crate::paths::{PathIndex, normalized};
use crate::scan::FileFacts;

const MANIFEST: &str = "Cargo.toml";
const DEFAULT_LIBRARY: &str = "src/lib.rs";
const BINARIES: &str = "src/bin/";
const TARGET_DIRS: [&str; 3] = ["tests/", "benches/", "examples/"];

pub(crate) struct RustTree<'p, 'a> {
    paths: &'p PathIndex<'a>,
    crates: BTreeMap<String, &'a str>,
    // Each package directory with its library root, the longest directory first.
    packages: Vec<(String, Option<&'a str>)>,
    manifests: bool,
    // The module file that each module file binds under a local name, from an earlier pass.
    pub(crate) exports: BTreeMap<(&'a str, &'a str), &'a str>,
}

// A walk position: a module file, or the directory of an inline module that holds module files.
enum Module<'a> {
    File(&'a str),
    Dir(String),
}

impl<'p, 'a> RustTree<'p, 'a> {
    pub(crate) fn build(files: &'a [FileFacts], paths: &'p PathIndex<'a>) -> RustTree<'p, 'a> {
        let mut crates = BTreeMap::new();
        let mut packages = Vec::new();
        let manifests: Vec<&FileFacts> = files.iter().filter(|file| is_manifest(file)).collect();
        for file in &manifests {
            let Some(manifest) = &file.facts.manifest else {
                continue;
            };
            let dir = file
                .path
                .strip_suffix(MANIFEST)
                .unwrap_or_default()
                .to_owned();
            let library = manifest.lib_path.as_deref().unwrap_or(DEFAULT_LIBRARY);
            let root = paths.get(&normalized(&format!("{dir}{library}")));
            let name = manifest.lib_name.as_ref().unwrap_or(&manifest.package);
            if let Some(root) = root {
                crates.entry(name.replace('-', "_")).or_insert(root);
            }
            packages.push((dir, root));
        }
        packages.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
        RustTree {
            paths,
            crates,
            packages,
            manifests: !manifests.is_empty(),
            exports: BTreeMap::new(),
        }
    }

    pub(crate) fn holds_manifest(&self) -> bool {
        self.manifests
    }

    // The root file of the crate target that holds `path`: a binary, test, bench or example
    // target is its own root, and any other file under the library's directory belongs to it.
    pub(crate) fn crate_root(&self, path: &'a str) -> Option<&'a str> {
        let (dir, library) = self
            .packages
            .iter()
            .find(|(dir, _)| path.starts_with(dir.as_str()))?;
        let inside = &path[dir.len()..];
        if let Some(target) = target_dir(inside) {
            return self.target_root(path, dir, target);
        }
        let main = self.paths.get(&format!("{dir}src/main.rs"));
        if main == Some(path) {
            return main;
        }
        if let Some(library) = library.filter(|library| holds(parent_dir(library), path)) {
            return Some(library);
        }
        if inside.starts_with("src/") {
            return main;
        }
        Some(path)
    }

    // A file directly in a target directory is its own root, and a subdirectory's root is its
    // `main.rs`.
    fn target_root(&self, path: &'a str, dir: &str, target: (&str, &str)) -> Option<&'a str> {
        let (top, rest) = target;
        match rest.split_once('/') {
            None => Some(path),
            Some((name, _)) => self.paths.get(&format!("{dir}{top}{name}/main.rs")),
        }
    }

    fn is_root(&self, path: &'a str) -> bool {
        self.crate_root(path) == Some(path)
    }

    // A crate root or `mod.rs` holds its child modules in its own directory, and `m.rs` in `m/`.
    fn module_dir(&self, path: &'a str) -> String {
        if self.is_root(path) || path.ends_with("/mod.rs") || path == "mod.rs" {
            parent_dir(path).to_owned()
        } else {
            path.strip_suffix(".rs").unwrap_or(path).to_owned()
        }
    }

    // The module that declares `module`: a module file, or an inline module's directory.
    fn parent(&self, module: &Module<'a>, importer: &'a str) -> Option<Module<'a>> {
        let root = self.crate_root(importer)?;
        let up = match module {
            Module::File(file) if *file == root => return None,
            Module::File(file) => parent_dir(&self.module_dir(file)).to_owned(),
            Module::Dir(dir) => parent_dir(dir).to_owned(),
        };
        if parent_dir(root) == up {
            return Some(Module::File(root));
        }
        self.module_at(up)
    }

    // The module whose children are in the directory `dir`: `dir/mod.rs`, else `dir.rs`, else
    // the directory itself as an inline module.
    fn module_at(&self, dir: String) -> Option<Module<'a>> {
        if dir.is_empty() {
            return self.paths.get("mod.rs").map(Module::File);
        }
        let file = self
            .paths
            .get(&format!("{dir}/mod.rs"))
            .or_else(|| self.paths.get(&format!("{dir}.rs")));
        Some(file.map_or(Module::Dir(dir), Module::File))
    }

    // The module file that `segments`, written in the file at `importer` inside `inline` inline
    // modules, walks to, and how many segments it used; `None` when the first names no module.
    pub(crate) fn resolve(
        &self,
        importer: &'a str,
        segments: &[&str],
        inline: u32,
    ) -> Option<(&'a str, usize)> {
        let lead = segments
            .iter()
            .take(inline as usize)
            .take_while(|segment| **segment == "super")
            .count();
        if lead > 0 {
            return self.resolve_in_file(importer, segments, lead);
        }
        let (start, used) = self.first(importer, segments)?;
        self.walk(importer, segments, start, used)
    }

    // The first `lead` segments are `super` hops that stay inside the importer's inline modules;
    // a `super` after them leaves the file as it would at the top of the file.
    fn resolve_in_file(
        &self,
        importer: &'a str,
        segments: &[&str],
        lead: usize,
    ) -> Option<(&'a str, usize)> {
        let rest = &segments[lead..];
        match rest.first() {
            None => Some((importer, lead)),
            Some(&"super") => self
                .resolve(importer, rest, 0)
                .map(|(file, used)| (file, used + lead)),
            Some(_) => {
                let inner: Vec<&str> = std::iter::once("self")
                    .chain(rest.iter().copied())
                    .collect();
                self.resolve(importer, &inner, 0)
                    .map(|(file, used)| (file, used - 1 + lead))
            }
        }
    }

    // Where the first segment starts the walk, and how many segments it used.
    fn first(&self, importer: &'a str, segments: &[&str]) -> Option<(Module<'a>, usize)> {
        let found = match *segments.first()? {
            "crate" | "$crate" => (Module::File(self.crate_root(importer)?), 1),
            "self" => (Module::File(importer), 1),
            "super" => {
                let mut current = Module::File(importer);
                let mut used = 0;
                while segments.get(used) == Some(&"super") {
                    current = self.parent(&current, importer)?;
                    used += 1;
                }
                (current, used)
            }
            "" => (Module::File(self.crates.get(*segments.get(1)?)?), 2),
            name => match self.step(&Module::File(importer), name) {
                Some(found) => (found, 1),
                None => (Module::File(self.crates.get(name)?), 1),
            },
        };
        Some(found)
    }

    // Each next segment that names a module moves down; the walk answers with the last module
    // file it reached.
    fn walk(
        &self,
        importer: &'a str,
        segments: &[&str],
        start: Module<'a>,
        mut used: usize,
    ) -> Option<(&'a str, usize)> {
        let mut last_file = match start {
            Module::File(file) => (file, used),
            Module::Dir(_) => (importer, 0),
        };
        let mut current = start;
        while let Some(next) = segments
            .get(used)
            .and_then(|segment| self.step(&current, segment))
        {
            current = next;
            used += 1;
            if let Module::File(file) = current {
                last_file = (file, used);
            }
        }
        match current {
            Module::File(file) => Some((file, used)),
            Module::Dir(_) => (last_file.1 > 0).then_some(last_file),
        }
    }

    // One segment down from `module`: a module file, else a module that the file binds by
    // `use`, else the directory of an inline module.
    fn step(&self, module: &Module<'a>, segment: &str) -> Option<Module<'a>> {
        let dir = match module {
            Module::File(file) => self.module_dir(file),
            Module::Dir(dir) => dir.clone(),
        };
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let found = self
            .paths
            .get(&format!("{prefix}{segment}.rs"))
            .or_else(|| self.paths.get(&format!("{prefix}{segment}/mod.rs")));
        if let Some(found) = found {
            return Some(Module::File(found));
        }
        if let Module::File(file) = module
            && let Some(bound) = self.exports.get(&(*file, segment))
        {
            return Some(Module::File(bound));
        }
        let inner = format!("{prefix}{segment}");
        self.paths
            .has_prefix(&format!("{inner}/"))
            .then_some(Module::Dir(inner))
    }

    // Whether a path's first segment names a module of this repository.
    pub(crate) fn names_repository_module(&self, importer: &'a str, first: &str) -> bool {
        matches!(first, "crate" | "$crate" | "self" | "super")
            || self.crates.contains_key(first)
            || matches!(
                self.step(&Module::File(importer), first),
                Some(Module::File(_))
            )
    }
}

// The binary, test, bench or example directory that a path below its package starts with, and
// the rest of the path.
fn target_dir(inside: &str) -> Option<(&'static str, &str)> {
    std::iter::once(BINARIES)
        .chain(TARGET_DIRS)
        .find_map(|top| inside.strip_prefix(top).map(|rest| (top, rest)))
}

fn is_manifest(file: &FileFacts) -> bool {
    file.language == Language::Toml
        && (file.path == MANIFEST || file.path.ends_with(&format!("/{MANIFEST}")))
}

// Whether `path` sits below the directory `dir`; every path sits below the root, "".
fn holds(dir: &str, path: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}
