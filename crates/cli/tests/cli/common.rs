//! What the cli tests share: a sandbox with its own HOME and caches, the scenario steps, one
//! run of the command line, and the fixture files.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use app::{EnvConfig, Services};
use cli::Streams;
use io::testing::Tree;
use serde_json::Value;

pub const PINNED: [(&str, &str); 6] = [
    ("GIT_AUTHOR_NAME", "cli tests"),
    ("GIT_AUTHOR_EMAIL", "cli@example.invalid"),
    ("GIT_AUTHOR_DATE", "2026-02-01T00:00:00Z"),
    ("GIT_COMMITTER_NAME", "cli tests"),
    ("GIT_COMMITTER_EMAIL", "cli@example.invalid"),
    ("GIT_COMMITTER_DATE", "2026-02-01T00:00:00Z"),
];

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
        let path = std::env::var_os("PATH").expect("the tests find git through PATH");
        let home = self.path().join("home");
        let vars = [
            ("PATH", path),
            ("HOME", home.clone().into_os_string()),
            ("XDG_CACHE_HOME", home.join(".cache").into_os_string()),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
        ];
        vars.map(|(name, value)| (OsString::from(name), value))
            .to_vec()
    }

    pub fn run(&self, args: &[OsString], cwd: &Path, columns: Option<&str>) -> Outcome {
        let env = self.env(cwd, columns);
        let services = Services::new(&env);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        let code = cli::run(args, &env, &services, &mut streams);
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

    // Runs one scenario step of crates/cli/fixtures/scenarios.json, as the generator ran it.
    pub fn step(&self, step: &[Value]) {
        let word = |at: usize| step[at].as_str().expect("a step word");
        let place = self.path().join(word(1));
        match word(0) {
            "write" => drop(self.write(word(1), word(2))),
            "init" => {
                self.tree
                    .create_dir(word(1))
                    .expect("a repository directory");
                self.git(&place, &["init", "--quiet", "-b", "main"]);
            }
            "commit" => {
                self.git(&place, &["add", "-A"]);
                self.git(&place, &["commit", "--quiet", "-m", "fixture"]);
            }
            other => panic!("no scenario step {other}"),
        }
    }

    pub fn git(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        let mut env = self.vars();
        env.extend(PINNED.map(|(name, value)| (OsString::from(name), OsString::from(value))));
        let mut words = vec![OsString::from("-C"), dir.as_os_str().to_owned()];
        words.extend(args.iter().map(OsString::from));
        let spec = io::Spec {
            program: OsString::from("git"),
            args: words,
            cwd: PathBuf::from("."),
            env,
            timeout: Duration::from_secs(30),
            stdout_cap: 1 << 24,
            stderr_cap: 1 << 16,
        };
        let outcome = io::run(&spec);
        let io::Outcome::Exited {
            status,
            stdout,
            stderr,
        } = outcome
        else {
            panic!("git did not finish: {outcome:?}");
        };
        let shown = String::from_utf8_lossy(&stderr);
        assert!(status.success(), "git {args:?}: {shown}");
        stdout
    }
}

// The cases of one section of crates/cli/fixtures/<file>.json.
pub fn cases(file: &str, section: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(format!("{file}.json"));
    let shown = path.display();
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let mut sections: serde_json::Map<String, Value> =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let cases = sections
        .remove(section)
        .unwrap_or_else(|| panic!("{shown}: no {section}"));
    cases
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("{shown}: {section} is no list"))
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
