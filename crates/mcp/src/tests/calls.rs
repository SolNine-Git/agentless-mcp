use prompts::envelope::RECEIPT_HEADER;
use serde_json::Value as Json;

use super::Sandbox;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
// Deviation D8: the reference fails on the home of an unknown user with its unplanned-error
// answer; this build reads `~name` as a relative path, which no allowlist holds.
const UNKNOWN_USER: &str = "~nosuchuser_zz/x";
const NOT_ALLOWED: &str = "Error calling tool 'orient': repository refused: the requested root is \
                           not one of this server's roots. Retry with repo_root set to one of: ";

// The answer this build gives in place of the reference answer: the refusal that lists the roots.
fn unknown_user_refusal(got: &str) -> String {
    let listing = got.strip_prefix(NOT_ALLOWED).unwrap_or("<no listing>");
    format!("{NOT_ALLOWED}{listing}")
}

fn runs() -> Vec<Json> {
    text::testing::cases(MANIFEST_DIR, "calls", "runs")
}

fn words(value: &Json) -> Vec<&str> {
    let list = value.as_array().expect("a word list");
    list.iter()
        .map(|word| word.as_str().expect("a word"))
        .collect()
}

// The receipt's cache field names each build's cache file, so both sides read a placeholder, and
// the capabilities report names agentless.db where the reference names tags.db (P2 D7).
pub(super) fn masked(text: &str) -> String {
    let lines: Vec<String> = text
        .replace("/<KEY>/tags.db", "/<KEY>/agentless.db")
        .split('\n')
        .map(|line| match line.split_once("   cache: ") {
            Some((head, _)) if line.starts_with(RECEIPT_HEADER) => {
                format!("{head}   cache: <CACHE>")
            }
            _ => line.to_owned(),
        })
        .collect();
    lines.join("\n")
}

// The arguments with the sandbox in place of its placeholder.
fn placed(sandbox: &Sandbox, arguments: &Json) -> Option<serde_json::Map<String, Json>> {
    let text = sandbox.placed(&arguments.to_string());
    let placed: Json = serde_json::from_str(&text).expect("arguments as JSON");
    placed.as_object().cloned()
}

// The run of the calls fixture named `name`.
pub(super) fn run_named(name: &str) -> Json {
    let found = runs().into_iter().find(|run| run["name"] == name);
    found.unwrap_or_else(|| panic!("no run {name}"))
}

// A sandbox after the steps of `run`.
pub(super) fn built(run: &Json) -> Sandbox {
    let sandbox = Sandbox::new();
    for step in run["steps"].as_array().expect("a step list") {
        sandbox.step(&words(step));
    }
    sandbox
}

// The masked reference answer of the call of `run` that makes `tool` with `arguments`.
pub(super) fn reference(run: &Json, tool: &str, arguments: &Json) -> String {
    let calls = run["calls"].as_array().expect("a call list");
    let found = calls
        .iter()
        .find(|item| item[0] == tool && item[1] == *arguments);
    let item = found.unwrap_or_else(|| panic!("no call {tool} {arguments}"));
    masked(item[2].as_str().expect("a text"))
}

// The masked answer of `server` to `tool` with `arguments`, and its error flag.
pub(super) fn answered(
    sandbox: &Sandbox,
    server: &crate::Server,
    tool: &str,
    arguments: &Json,
) -> (String, bool) {
    let answer = server.answer(tool, placed(sandbox, arguments));
    (masked(&sandbox.normalised(&answer.text)), answer.is_error)
}

// Each call of `run` that answers otherwise than the fixture says, with both answers.
fn differences(run: &Json) -> Vec<String> {
    let sandbox = built(run);
    let server = sandbox.server(
        &words(&run["roots"]),
        &words(&run["roots_files"]),
        run["launch"].as_str(),
    );
    let mut failed = Vec::new();
    for item in run["calls"].as_array().expect("a call list") {
        if item[0] == "step" {
            sandbox.step(&words(item)[1..]);
            continue;
        }
        let name = item[0].as_str().expect("a tool name");
        let got = answered(&sandbox, &server, name, &item[1]);
        let want = if item[1]["repo_root"] == UNKNOWN_USER {
            (unknown_user_refusal(&got.0), true)
        } else {
            (masked(item[2].as_str().expect("a text")), item[3] == true)
        };
        if got != want {
            failed.push(format!(
                "{} {name} {}\n got {got:?}\nwant {want:?}",
                run["name"], item[1]
            ));
        }
    }
    failed
}

#[test]
fn each_call_answers_as_the_reference_says() {
    let runs = runs();
    assert_eq!(runs.len(), 7);
    let failed: Vec<String> = runs.iter().flat_map(differences).collect();
    assert!(
        failed.is_empty(),
        "{} calls differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
