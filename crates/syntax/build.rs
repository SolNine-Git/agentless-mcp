//! Unpack the vendored grammar archive into `OUT_DIR` and compile its C sources into one static library.

use std::collections::BTreeSet;
use std::env;
use std::fs::File;
use std::io::{BufReader, Read};
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
    let archive = manifest_dir.join(ARCHIVE);
    unpack(&archive, &root);
    let unpacked = Unpacked {
        members: members(&archive),
        root,
    };
    let objects = compile_grammars(&unpacked, jobs());
    let mut library = base();
    utf8proc(&mut library, &unpacked.root);
    library
        .objects(objects)
        .file(unpacked.root.join("shim/utf8proc/utf8proc.c"))
        .cargo_metadata(true)
        .compile(LIBRARY);
}

// The unpack root and the archive's paths below it. OUT_DIR outlives a grammar bump, so a file that
// only an older archive held can still be on disk: a compile decision asks the archive, not the disk.
struct Unpacked {
    root: PathBuf,
    members: BTreeSet<PathBuf>,
}

impl Unpacked {
    fn holds(&self, path: &Path) -> bool {
        self.members.iter().any(|member| member.starts_with(path))
    }
}

fn jobs() -> usize {
    env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse().ok())
        .unwrap_or(1)
}

fn tar_stream(archive: &Path) -> tar::Archive<impl Read> {
    let file = File::open(archive).unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
    let decoder = ruzstd::decoding::StreamingDecoder::new(BufReader::new(file))
        .unwrap_or_else(|error| panic!("{}: not a zstd frame: {error}", archive.display()));
    tar::Archive::new(decoder)
}

fn unpack(archive: &Path, into: &Path) {
    // OUT_DIR belongs to this build, so the unpack writes nothing outside the build directory.
    tar_stream(archive)
        .unpack(into)
        .unwrap_or_else(|error| panic!("{}: unpack failed: {error}", archive.display()));
}

fn members(archive: &Path) -> BTreeSet<PathBuf> {
    let mut stream = tar_stream(archive);
    let entries = stream
        .entries()
        .unwrap_or_else(|error| panic!("{}: not a tar stream: {error}", archive.display()));
    entries
        .map(|entry| {
            let entry = entry.unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
            let path = entry.path();
            path.unwrap_or_else(|error| panic!("{}: {error}", archive.display()))
                .into_owned()
        })
        .collect()
}

fn compile_grammars(unpacked: &Unpacked, jobs: usize) -> Vec<PathBuf> {
    let next = AtomicUsize::new(0);
    let compiled: Compiled = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, GRAMMARS.len()) {
            scope.spawn(|| worker(unpacked, &next, &compiled));
        }
    });
    let mut compiled = compiled.into_inner().expect("a compile thread panicked");
    compiled.sort();
    compiled
        .into_iter()
        .flat_map(|(_, objects)| objects)
        .collect()
}

fn worker(unpacked: &Unpacked, next: &AtomicUsize, compiled: &Compiled) {
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(name) = GRAMMARS.get(index) else {
            return;
        };
        let objects = grammar_objects(unpacked, name);
        compiled
            .lock()
            .expect("a compile thread panicked")
            .push((index, objects));
    }
}

fn grammar_objects(unpacked: &Unpacked, name: &str) -> Vec<PathBuf> {
    let dir = Path::new("parsers").join(name);
    let mut parser = grammar_build(unpacked, &dir);
    parser.file(unpacked.root.join(&dir).join("src/parser.c"));
    let mut objects = parser.compile_intermediates();
    let scanner = dir.join("src/scanner.c");
    if unpacked.holds(&scanner) {
        let mut build = grammar_build(unpacked, &dir);
        shim(&mut build, &unpacked.root);
        build.file(unpacked.root.join(scanner));
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

fn grammar_build(unpacked: &Unpacked, dir: &Path) -> cc::Build {
    let mut build = base();
    let source = unpacked.root.join(dir);
    build.flag(format!("-I{}", source.join("src").display()));
    if unpacked.holds(&dir.join("common")) {
        build.flag(format!("-I{}", source.join("common").display()));
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
