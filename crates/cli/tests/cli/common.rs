//! What the cli tests share: a sandbox with its own HOME and caches, the scenario steps, one
//! run of the command line, and the fixture files.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use app::{EnvConfig, Services};
use cli::Streams;
use io::testing::{Identity, Tree};
use serde_json::Value;

// The commit identity of every setup repository. The cli generator pins the same one.
const IDENTITY: Identity = Identity {
    name: "cli tests",
    email: "cli@example.invalid",
    date: "2026-02-01T00:00:00Z",
};

#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub out: String,
    pub err: String,
    pub code: u8,
}

pub struct Sandbox {
    tree: Tree,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let tree = Tree::new().unwrap_or_else(|error| panic!("a temporary tree: {error}"));
        tree.create_dir("home").expect("a home directory");
        Sandbox { tree }
    }

    pub fn path(&self) -> &Path {
        self.tree.path()
    }

    // The start-up environment: PATH to find git, and HOME and the caches inside the sandbox.
    pub fn env(&self, cwd: &Path, columns: Option<&str>) -> EnvConfig {
        let mut vars = self.vars();
        vars.extend(columns.map(|value| (OsString::from("COLUMNS"), OsString::from(value))));
        EnvConfig::new(vars, cwd.to_path_buf())
    }

    pub fn vars(&self) -> Vec<(OsString, OsString)> {
        io::testing::hermetic_env(&self.path().join("home"))
    }

    pub fn run(&self, args: &[OsString], cwd: &Path, columns: Option<&str>) -> Outcome {
        self.run_with_stdin(args, cwd, columns, &mut &b""[..])
    }

    pub fn run_with_stdin(
        &self,
        args: &[OsString],
        cwd: &Path,
        columns: Option<&str>,
        stdin: &mut dyn Read,
    ) -> Outcome {
        let env = self.env(cwd, columns);
        let services = Services::new(&env);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        let code = cli::run(args, &env, &services, stdin, &mut streams);
        Outcome {
            out: String::from_utf8(out).expect("UTF-8 stdout"),
            err: String::from_utf8(err).expect("UTF-8 stderr"),
            code,
        }
    }

    pub fn write(&self, relative: &str, text: &str) -> PathBuf {
        if let Some((parent, _)) = relative.rsplit_once('/') {
            self.tree.create_dir(parent).expect("a parent directory");
        }
        self.tree
            .write_file(relative, text)
            .expect("a written file")
    }

    // A file whose name is the latin-1 bytes of `relative`, so a character above U+007F is no
    // UTF-8.
    pub fn write_latin(&self, relative: &str, text: &str) -> PathBuf {
        if let Some((parent, _)) = relative.rsplit_once('/') {
            self.tree.create_dir(parent).expect("a parent directory");
        }
        let name = OsString::from_vec(io::testing::latin(relative));
        self.tree.write_file(name, text).expect("a written file")
    }

    // Runs one scenario step of crates/cli/fixtures/scenarios.json, as the generator ran it.
    pub fn step(&self, step: &[Value]) {
        let word = |at: usize| step[at].as_str().expect("a step word");
        let place = self.path().join(word(1));
        match word(0) {
            "write" => drop(self.write(word(1), word(2))),
            "latin" => drop(self.write_latin(word(1), word(2))),
            "init" => {
                self.tree
                    .create_dir(word(1))
                    .expect("a repository directory");
                self.git(&place, &["init", "--quiet", "-b", "main"]);
            }
            "commit" => {
                self.git(&place, &["add", "-A"]);
                let message = step.get(2).and_then(Value::as_str).unwrap_or("fixture");
                self.git(&place, &["commit", "--quiet", "-m", message]);
            }
            other => panic!("no scenario step {other}"),
        }
    }

    pub fn git(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        io::testing::run_git(dir, args, &self.vars(), &IDENTITY)
    }
}

// The cases of one section of crates/cli/fixtures/<file>.json.
pub fn cases(file: &str, section: &str) -> Vec<Value> {
    io::testing::cases(env!("CARGO_MANIFEST_DIR"), file, section)
}

pub fn words(value: &Value) -> Vec<String> {
    let list = value.as_array().expect("a word list");
    list.iter()
        .map(|word| word.as_str().expect("a word").to_owned())
        .collect()
}

pub fn expected(case: &Value) -> Outcome {
    let text = |key: &str| case[key].as_str().expect("an output text").to_owned();
    let code = case["code"].as_u64().expect("an exit code");
    Outcome {
        out: text("out"),
        err: text("err"),
        code: u8::try_from(code).expect("a code below 256"),
    }
}

// The sandbox, the cache key, the version and the grammar cache as placeholders, by the rule
// that the generator applies to the Python output.
pub fn normalised(text: &str, sandbox: &Path) -> String {
    let text = text.replace(&sandbox.display().to_string(), "<SANDBOX>");
    let text = key_placeholders(&text);
    let lines: Vec<String> = text.split('\n').map(normal_line).collect();
    lines.join("\n")
}

fn key_placeholders(text: &str) -> String {
    const MARK: &str = "<SANDBOX>/home/.cache/agentless-mcp/";
    let mut done = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(MARK) {
        let (head, tail) = rest.split_at(at + MARK.len());
        done.push_str(head);
        let end = tail.find(|c: char| c == '/' || c == '"' || c.is_whitespace());
        let end = end.unwrap_or(tail.len());
        if end > 0 && tail[end..].starts_with('/') {
            done.push_str("<KEY>");
            rest = &tail[end..];
        } else {
            rest = tail;
        }
    }
    done.push_str(rest);
    done
}

fn normal_line(line: &str) -> String {
    let stripped = line.trim_start_matches(' ');
    let indent = &line[..line.len() - stripped.len()];
    let comma = if line.ends_with(',') { "," } else { "" };
    if line == format!("agentless-mcp {}", env!("CARGO_PKG_VERSION")) {
        return "agentless-mcp <VERSION>".to_owned();
    }
    if line.starts_with("pack ")
        && let Some((head, _)) = line.split_once("  grammar cache ")
    {
        return format!("{head}  grammar cache <GRAMMAR_CACHE>");
    }
    if stripped.starts_with("\"version\": \"") {
        return format!("{indent}\"version\": \"<VERSION>\"{comma}");
    }
    if stripped.starts_with("\"grammar_cache\": \"") {
        return format!("{indent}\"grammar_cache\": \"<GRAMMAR_CACHE>\"{comma}");
    }
    line.to_owned()
}
