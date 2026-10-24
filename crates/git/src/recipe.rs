// The history fixture's repositories, built from the recipes the fixture stores, as the generator
// built them.

use std::ffi::OsStr;
use std::fmt::Write;
use std::path::PathBuf;

use io::testing::Identity;
use serde_json::Value;

use crate::testing::{Scratch, cases, latin};

pub(crate) const FIXTURE: &str = "history/history";
const ROOT_MARK: &str = "{root}";

type Step = Vec<Value>;

pub(crate) struct Repos {
    pub(crate) scratch: Scratch,
    name: &'static str,
    email: &'static str,
}

impl Repos {
    pub(crate) fn build() -> Self {
        let [(name, email)]: [(String, String); 1] = cases(FIXTURE, "identity");
        let repos = Self {
            scratch: Scratch::new(),
            name: leak(name),
            email: leak(email),
        };
        let recipes: Vec<(String, Vec<Step>)> = cases(FIXTURE, "repos");
        for (repo, steps) in &recipes {
            repos.build_repo(repo, steps);
        }
        repos
    }

    pub(crate) fn path(&self, repo: &str) -> PathBuf {
        self.scratch.root.join(repo)
    }

    // The note with the scratch root as the generator wrote it.
    pub(crate) fn unrooted(&self, note: &str) -> String {
        let root = self.scratch.root.to_str().expect("a UTF-8 scratch path");
        note.replace(root, ROOT_MARK)
    }

    fn build_repo(&self, repo: &str, steps: &[Step]) {
        let first = steps[0][0].as_str();
        if !matches!(first, Some("mkdir" | "clone_shallow")) {
            self.scratch.init(repo);
        }
        for step in steps {
            self.step(repo, step);
        }
    }

    fn step(&self, repo: &str, step: &[Value]) {
        let text = |index: usize| -> &str {
            step[index]
                .as_str()
                .unwrap_or_else(|| panic!("{step:?}: no text at {index}"))
        };
        match text(0) {
            "mkdir" => {
                self.scratch.create_dir(repo);
            }
            "clone_shallow" => self.clone_shallow(repo, text(1)),
            "write" => {
                self.scratch.write(&format!("{repo}/{}", text(1)), text(2));
            }
            "commit" => self.commit(repo, &latin(text(1)), text(2)),
            "commit_lines" => {
                let count = step[2].as_u64().expect("a line count");
                self.commit(repo, &message_lines(text(1), count), text(3));
            }
            other => panic!("unknown recipe step {other:?}"),
        }
    }

    fn clone_shallow(&self, repo: &str, source: &str) {
        let uri = format!("file://{}", self.path(source).display());
        let target = self.path(repo);
        let target = target.to_str().expect("a UTF-8 scratch path");
        let args = ["clone", "--quiet", "--depth", "1", &uri, target];
        self.scratch.run_git(&self.scratch.root, &args);
    }

    fn commit(&self, repo: &str, message: &[u8], date: &str) {
        let message_file = self.scratch.write_bytes(OsStr::new("message"), message);
        let message_file = message_file.to_str().expect("a UTF-8 scratch path");
        let identity = Identity {
            name: self.name,
            email: self.email,
            date: leak(date.to_owned()),
        };
        let root = self.path(repo);
        let env = self.scratch.env();
        let run = |args: &[&str]| io::testing::run_git(&root, args, &env, &identity);
        run(&["add", "-A"]);
        run(&[
            "commit",
            "--quiet",
            "--cleanup=verbatim",
            "-F",
            message_file,
        ]);
    }
}

fn message_lines(subject: &str, count: u64) -> Vec<u8> {
    let mut message = format!("{subject}\n\n");
    for number in 0..count {
        writeln!(message, "line {number:07}").expect("writing to a String cannot fail");
    }
    message.into_bytes()
}

// Identity takes static text, and a test builds few repositories.
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}
