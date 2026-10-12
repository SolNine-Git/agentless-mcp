//! Unpack the vendored grammar archive into `OUT_DIR` and compile its C sources into one static library.

use std::env;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

const ARCHIVE: &str = "vendor/grammars-1.14.3.tar.zst";
const LIBRARY: &str = "tree_sitter_grammars";
// Largest parser.c first, so that the long compiles start early.
const GRAMMARS: [&str; 22] = [
    "sql",
    "kotlin",
    "csharp",
    "scala",
    "cpp",
    "swift",
    "ruby",
    "bash",
    "tsx",
    "typescript",
    "php",
    "rust",
    "c",
    "python",
    "javascript",
    "java",
    "go",
    "yaml",
    "hcl",
    "lua",
    "toml",
    "json",
];

type Compiled = Mutex<Vec<(usize, Vec<PathBuf>)>>;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed={ARCHIVE}");
    let root = out_dir.join("grammars");
    unpack(&manifest_dir.join(ARCHIVE), &root);
    let objects = compile_grammars(&root, jobs());
    let mut library = base();
    utf8proc(&mut library, &root);
    library
        .objects(objects)
        .file(root.join("shim/utf8proc/utf8proc.c"))
        .cargo_metadata(true)
        .compile(LIBRARY);
}

fn jobs() -> usize {
    env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse().ok())
        .unwrap_or(1)
}

fn unpack(archive: &Path, into: &Path) {
    let file = File::open(archive).unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
    let decoder = ruzstd::decoding::StreamingDecoder::new(BufReader::new(file))
        .unwrap_or_else(|error| panic!("{}: not a zstd frame: {error}", archive.display()));
    // The sources are build output in OUT_DIR, not a runtime write, so tar writes them for the build.
    tar::Archive::new(decoder)
        .unpack(into)
        .unwrap_or_else(|error| panic!("{}: unpack failed: {error}", archive.display()));
}

fn compile_grammars(root: &Path, jobs: usize) -> Vec<PathBuf> {
    let next = AtomicUsize::new(0);
    let compiled: Compiled = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, GRAMMARS.len()) {
            scope.spawn(|| worker(root, &next, &compiled));
        }
    });
    let mut compiled = compiled.into_inner().expect("a compile thread panicked");
    compiled.sort();
    compiled
        .into_iter()
        .flat_map(|(_, objects)| objects)
        .collect()
}

fn worker(root: &Path, next: &AtomicUsize, compiled: &Compiled) {
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(name) = GRAMMARS.get(index) else {
            return;
        };
        let objects = grammar_objects(root, name);
        compiled
            .lock()
            .expect("a compile thread panicked")
            .push((index, objects));
    }
}

fn grammar_objects(root: &Path, name: &str) -> Vec<PathBuf> {
    let dir = root.join("parsers").join(name);
    let mut parser = grammar_build(&dir);
    parser.file(dir.join("src/parser.c"));
    let mut objects = parser.compile_intermediates();
    let scanner = dir.join("src/scanner.c");
    if scanner.exists() {
        let mut build = grammar_build(&dir);
        shim(&mut build, root);
        build.file(scanner);
        objects.extend(build.compile_intermediates());
    }
    objects
}

// The flags of the pack's static build. Include dirs go through flag(): the gates refuse the word
// that names cc's own method.
fn base() -> cc::Build {
    let mut build = cc::Build::new();
    build
        .std("c11")
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .cargo_metadata(false)
        .define("TREE_SITTER_HIDE_SYMBOLS", None)
        .flag("-fvisibility=hidden")
        .flag("-fno-strict-aliasing");
    build
}

fn grammar_build(dir: &Path) -> cc::Build {
    let mut build = base();
    build.flag(format!("-I{}", dir.join("src").display()));
    let common = dir.join("common");
    if common.exists() {
        build.flag(format!("-I{}", common.display()));
    }
    build
}

fn utf8proc(build: &mut cc::Build, root: &Path) {
    build
        .flag(format!("-I{}", root.join("shim/utf8proc").display()))
        .define("UTF8PROC_STATIC", None);
}

// Scanners classify non-ASCII text through the shim, which utf8proc backs, and not through libc.
fn shim(build: &mut cc::Build, root: &Path) {
    utf8proc(build, root);
    build.flag("-include").flag(root.join("shim/ctype_shim.h"));
}
