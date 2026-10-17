mod calls;
mod deadline;
mod roots;
mod wire;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use app::{EnvConfig, Services, Session};
use io::testing::{Identity, Tree, hermetic_env, run_git};

use crate::dispatch::{answered, deadline, prepared};
use crate::{Arming, Log, Ports, RootsFile, Server, ServerConfig};

const IDENTITY: Identity = Identity {
    name: "cli tests",
    email: "cli@example.invalid",
    date: "2026-02-01T00:00:00Z",
};
const SANDBOX: &str = "<SANDBOX>";
pub(crate) const CALL_LIMIT: f64 = 30.0;
pub(crate) const RECACHE_GRACE: f64 = 60.0;

// The roots, the roots files and the launch directory of a test server, below its sandbox.
#[derive(Clone, Copy)]
pub(crate) struct Setup<'a> {
    pub(crate) roots: &'a [&'a str],
    pub(crate) files: &'a [&'a str],
    pub(crate) launch: Option<&'a str>,
}

// A scratch directory with a home, where the steps of a fixture run build repositories.
pub(crate) struct Sandbox {
    tree: Tree,
    root: PathBuf,
}

impl Sandbox {
    pub(crate) fn new() -> Sandbox {
        let tree = Tree::new().expect("a scratch directory");
        let root = io::resolve(tree.path()).expect("a resolvable scratch directory");
        tree.create_dir("home").expect("a home directory");
        Sandbox { tree, root }
    }

    pub(crate) fn placed(&self, text: &str) -> String {
        text.replace(SANDBOX, &self.root.display().to_string())
    }

    fn vars(&self) -> Vec<(OsString, OsString)> {
        let mut vars = hermetic_env(&self.root.join("home"));
        vars.push((OsString::from("LANG"), OsString::from("C.UTF-8")));
        vars
    }

    pub(crate) fn services(&self) -> Arc<Services> {
        Arc::new(Services::new(&EnvConfig::new(
            self.vars(),
            self.root.clone(),
        )))
    }

    // One step of a fixture run, as the generator runs it.
    pub(crate) fn step(&self, step: &[&str]) {
        let place = self.root.join(step[1]);
        match step[0] {
            "write" => self.written(step[1], step[2].as_bytes()),
            "bytes" => self.written(step[1], &io::testing::latin(step[2])),
            "roots" => {
                let lines: Vec<String> = step[2..].iter().map(|line| self.placed(line)).collect();
                let text: String = lines
                    .iter()
                    .flat_map(|line| [line.as_str(), "\n"])
                    .collect();
                self.written(step[1], text.as_bytes());
            }
            "remove" => assert!(io::remove_file(&place).expect("a removal"), "{step:?}"),
            "init" => {
                self.tree
                    .create_dir(step[1])
                    .expect("a repository directory");
                drop(run_git(
                    &place,
                    &["init", "--quiet", "-b", "main"],
                    &self.vars(),
                    &IDENTITY,
                ));
            }
            "commit" => {
                let message = step.get(2).copied().unwrap_or("fixture");
                drop(run_git(&place, &["add", "-A"], &self.vars(), &IDENTITY));
                let commit = ["commit", "--quiet", "-m", message];
                drop(run_git(&place, &commit, &self.vars(), &IDENTITY));
            }
            other => panic!("no fixture step {other}"),
        }
    }

    fn written(&self, place: &str, bytes: &[u8]) {
        if let Some((parent, _)) = place.rsplit_once('/') {
            self.tree.create_dir(parent).expect("a parent directory");
        }
        self.tree.write_file(place, bytes).expect("a written file");
    }

    // The server of a fixture run: its roots and roots files below the sandbox.
    pub(crate) fn server(&self, roots: &[&str], files: &[&str], launch: Option<&str>) -> Server {
        let setup = Setup {
            roots,
            files,
            launch,
        };
        self.server_with(&setup, self.services(), quiet_ports())
    }

    pub(crate) fn server_with(
        &self,
        setup: &Setup<'_>,
        services: Arc<Services>,
        ports: Ports,
    ) -> Server {
        let Setup {
            roots,
            files,
            launch,
        } = *setup;
        let resolved = |place: &str| io::resolve(&self.root.join(place)).expect("a root");
        let roots_files = files
            .iter()
            .map(|file| RootsFile::read(&self.root.join(file), &services).expect("a roots file"))
            .collect();
        let config = ServerConfig {
            roots: roots.iter().map(|root| resolved(root)).collect(),
            roots_files,
            launch_dir: launch.map(|place| self.root.join(place)),
            max_concurrency: 8,
            call_limit: CALL_LIMIT,
            recache_grace: RECACHE_GRACE,
        };
        Server::new(config, services, ports)
    }

    // The sandbox, the cache key, the version and the grammar cache as placeholders.
    pub(crate) fn normalised(&self, text: &str) -> String {
        let text = text.replace(&self.root.display().to_string(), SANDBOX);
        let lines: Vec<String> = text.split('\n').map(normal_line).collect();
        keyed(&lines.join("\n"))
    }
}

fn normal_line(line: &str) -> String {
    if line == format!("agentless-mcp {}", env!("CARGO_PKG_VERSION")) {
        return "agentless-mcp <VERSION>".to_owned();
    }
    match line.split_once("  grammar cache ") {
        Some((head, _)) if line.starts_with("pack ") => {
            format!("{head}  grammar cache <GRAMMAR_CACHE>")
        }
        _ => line.to_owned(),
    }
}

// The directory of each cache key below the sandbox home reads <KEY>.
fn keyed(text: &str) -> String {
    const MARK: &str = "<SANDBOX>/home/.cache/agentless-mcp/";
    let mut done = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(MARK) {
        let (head, tail) = rest.split_at(at + MARK.len());
        done.push_str(head);
        let end = tail.find(['/', '"', ' ', '\n']).unwrap_or(tail.len());
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

struct NoArming;

impl Arming for NoArming {
    fn arm(&self, _session: &Session) {}
}

// The log lines of a test server, kept for the assertions.
#[derive(Default)]
pub(crate) struct Kept {
    lines: Mutex<Vec<String>>,
}

impl Kept {
    pub(crate) fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("an unpoisoned log").clone()
    }
}

impl Log for Kept {
    fn line(&self, text: &str) {
        self.lines
            .lock()
            .expect("an unpoisoned log")
            .push(text.to_owned());
    }
}

pub(crate) fn quiet_ports() -> Ports {
    Ports {
        arming: Arc::new(NoArming),
        log: Arc::new(Kept::default()),
    }
}

// The text of one answer and whether it reports an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolAnswer {
    pub(crate) text: String,
    pub(crate) is_error: bool,
}

impl Server {
    // The answer of one call on the calling thread, as the call's own thread computes it.
    pub(crate) fn answer(
        &self,
        name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> ToolAnswer {
        let inner = &*self.inner;
        let answer = match prepared(inner, name, arguments) {
            Ok(call) => answered(inner, &call, deadline(inner, &call).as_ref()),
            Err(refused) => refused,
        };
        ToolAnswer {
            text: answer.text,
            is_error: answer.error,
        }
    }
}
