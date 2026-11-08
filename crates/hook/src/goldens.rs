use std::path::{Path, PathBuf};

use app::{EnvConfig, Services};
use io::testing::{Identity, run_git};
use serde_json::Value;

use crate::testing::{MANIFEST_DIR, Sandbox};

const REQUEST: &str = ".request.json";
const STDOUT: &str = "stdout";
const CONTEXT: &str = "txt";
const PLACEHOLDER: &str = "<REPO>";
// The render goldens' commit, so that a card's last change names the commit that theirs do.
const IDENTITY: Identity = Identity {
    name: "render goldens",
    email: "render@example.invalid",
    date: "2026-02-01T00:00:00Z",
};
// A late render leaves a hook silent, and a late git gives a card that says so; a retry in a new
// sandbox reads what the golden holds.
const TRIES: usize = 3;

fn characterization(parts: &[&str]) -> PathBuf {
    let base = [MANIFEST_DIR, "../../tests/characterization"];
    base.iter().chain(parts).collect()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// Each request of the hooks folder, by its golden's stem, in name order.
fn requests() -> Vec<(String, Value)> {
    let dir = characterization(&["goldens", "hooks"]);
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    let mut found: Vec<(String, Value)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let stem = path
                .file_name()?
                .to_str()?
                .strip_suffix(REQUEST)?
                .to_owned();
            let request = serde_json::from_str(&read(&path));
            Some((
                stem,
                request.unwrap_or_else(|error| panic!("{}: {error}", path.display())),
            ))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn golden(stem: &str, suffix: &str) -> Option<String> {
    let path = characterization(&["goldens", "hooks", &format!("{stem}.{suffix}")]);
    path.is_file().then(|| read(&path))
}

// A copy of one fixture repository, committed with the render goldens' identity, and the
// environment of a hook that runs in it: TMPDIR in the sandbox, and no log variable.
struct Rig {
    sandbox: Sandbox,
    env: EnvConfig,
    services: Services,
}

impl Rig {
    fn new(fixture: &str) -> Self {
        let sandbox = Sandbox::new();
        let root = sandbox.mkdir("repo");
        copied(&sandbox, &characterization(&["fixtures", fixture]), "repo");
        let tmp = sandbox.mkdir("tmp").into_os_string();
        let ceiling = sandbox.path().as_os_str().to_owned();
        let vars = sandbox.env(&[("TMPDIR", tmp), ("GIT_CEILING_DIRECTORIES", ceiling)]);
        let vars = vars.vars().to_vec();
        for args in [
            &["init", "--quiet", "-b", "main"][..],
            &["add", "-A"],
            &["commit", "--quiet", "-m", "fixture"],
        ] {
            drop(run_git(&root, args, &vars, &IDENTITY));
        }
        let env = EnvConfig::new(vars, root);
        let services = Services::new(&env);
        Self {
            sandbox,
            env,
            services,
        }
    }

    fn root(&self) -> String {
        self.sandbox.root().display().to_string()
    }

    // The hook's stdout for `payload`, with the repository's path as the placeholder.
    fn run(&self, hook: &str, payload: &Value) -> String {
        let stdin = payload.to_string().replace(PLACEHOLDER, &self.root());
        let (env, services) = (&self.env, &self.services);
        let mut stdin = stdin.as_bytes();
        let mut stdout = Vec::new();
        let code = if hook == "erosion" {
            let output = crate::erosion(env, services, &mut stdin);
            stdout = output.stdout.into_bytes();
            output.code
        } else {
            let found = crate::ContextHook::named(hook);
            let found = found.unwrap_or_else(|| panic!("no hook {hook}"));
            found.run(env, services, &mut stdin, &mut stdout)
        };
        assert_eq!(code, 0, "{hook}");
        let stdout = String::from_utf8(stdout).expect("UTF-8 stdout");
        stdout.replace(&self.root(), PLACEHOLDER)
    }
}

fn copied(sandbox: &Sandbox, from: &Path, below: &str) {
    let entries = std::fs::read_dir(from).unwrap_or_else(|error| panic!("{error}"));
    for entry in entries {
        let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        let relative = format!("{below}/{name}");
        if path.is_dir() {
            drop(sandbox.mkdir(&relative));
            copied(sandbox, &path, &relative);
        } else {
            let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{error}"));
            drop(sandbox.put(&relative, bytes));
        }
    }
}

// The runs before the request's own, then its own stdout.
fn stdout_of(request: &Value) -> String {
    let fixture = request["fixture"].as_str().expect("a fixture");
    let rig = Rig::new(fixture);
    for before in request["before"].as_array().into_iter().flatten() {
        let hook = before["hook"].as_str().expect("a hook");
        drop(rig.run(hook, &before["payload"]));
    }
    let hook = request["hook"].as_str().expect("a hook");
    rig.run(hook, &request["payload"])
}

fn context_of(stdout: &str) -> String {
    let reply: Value = serde_json::from_str(stdout).unwrap_or_else(|error| panic!("{error}"));
    let context = reply["hookSpecificOutput"]["additionalContext"].as_str();
    context.expect("a context string").to_owned()
}

#[test]
fn each_hook_stdout_and_context_match_their_goldens() {
    let requests = requests();
    assert_eq!(requests.len(), 8);
    for (stem, request) in requests {
        let expected = golden(&stem, STDOUT).unwrap_or_else(|| panic!("{stem}: no stdout golden"));
        let late = |stdout: &str| stdout.is_empty() || stdout.contains("last change: not read");
        let mut stdout = stdout_of(&request);
        for _ in 1..TRIES {
            if !late(&stdout) || late(&expected) {
                break;
            }
            stdout = stdout_of(&request);
        }
        assert_eq!(stdout, expected, "{stem}.{STDOUT}");
        let context = golden(&stem, CONTEXT);
        let shown = (!stdout.is_empty()).then(|| context_of(&stdout));
        assert_eq!(shown, context, "{stem}.{CONTEXT}");
    }
}

#[test]
fn every_hook_golden_has_its_request() {
    let dir = characterization(&["goldens", "hooks"]);
    let stems: Vec<String> = requests().into_iter().map(|(stem, _)| stem).collect();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{error}")) {
        let name = entry.unwrap_or_else(|error| panic!("{error}")).file_name();
        let name = name.to_string_lossy();
        if name.ends_with(REQUEST) {
            continue;
        }
        let stem = name.rsplit_once('.').map_or(&*name, |(stem, _)| stem);
        assert!(
            stems.iter().any(|known| known == stem),
            "{name}: no request"
        );
    }
}
